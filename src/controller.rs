use crate::utils::convert_label_selector_to_query_string;
use crate::vpa::VerticalPodAutoscalerTargetRef;
use futures::StreamExt;
use kube::api::{Patch, PatchParams};
use kube::core::{DynamicObject, GroupVersionKind};
use kube::discovery::ApiResource;
use kube::{Api, Resource, ResourceExt};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

use crate::{
    utils,
    vpa::{VerticalPodAutoscaler, VerticalPodAutoscalerSpec},
};
use k8s_openapi::{
    apimachinery::pkg::apis::meta::v1::LabelSelector, apimachinery::pkg::apis::meta::v1::ObjectMeta,
};
use kube::{
    api::ListParams,
    runtime::{
        controller::Action,
        reflector::{ObjectRef, Store},
        watcher::Config,
        Controller,
    },
    Client, CustomResource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::*;

struct Ctx {
    client: Client,
    gvks: Vec<GroupVersionKind>,
}
// Define the AutoVPA CRD struct
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[kube(group = "autovpa.dev", version = "v1", kind = "AutoVPA", namespaced)]
pub struct AutoVPASpec {
    selector: LabelSelector,
    vpa_template: VerticalPodAutoscalerTemplateSpec,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct VerticalPodAutoscalerTemplateSpec {
    /// Standard object's metadata. More info: https://git.k8s.io/community/contributors/devel/sig-architecture/api-conventions.md#metadata
    metadata: Option<ObjectMeta>,
    template: VerticalPodAutoscalerSpec,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("LabelSelector is invalid: {0}")]
    InvalidLabelSelector(String),

    #[error("Failed to get owner ref")]
    InvalidOwnerRef(),

    #[error("MissingObjectKey: {0}")]
    MissingObjectKey(&'static str),

    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
    },
    #[error("Serde error: {source}")]
    SerdeError {
        #[from]
        source: serde_yaml::Error,
    },
    // #[backtrace]
    // backtrace: Backtrace,  // automatically detected
}

type Result<T, E = Error> = std::result::Result<T, E>;

// Define the main function
#[tokio::main]
pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let gvks = vec![
        GroupVersionKind::gvk("apps", "v1", "Deployment"),
        GroupVersionKind::gvk("apps", "v1", "StatefulSet"),
        GroupVersionKind::gvk("apps", "v1", "DaemonSet"),
        GroupVersionKind::gvk("batch", "v1", "Job"),
    ];

    let client = Client::try_default().await.expect("failed to create kube client");

    //TODO: use namespaced api?
    let gen_api: Api<AutoVPA> = Api::all(client.clone());
    let vpa_api: Api<VerticalPodAutoscaler> = Api::all(client.clone());

    // In sceniro of oam controlled contrllers, there is a oam.dev.namespace label in the generated deployment | statefulset...
    if let Err(e) = gen_api.list(&ListParams::default().limit(1)).await {
        error!("autovpa crd is not querable; {e:?}, is the crd intalled?");
        info!("Installation: cargo run --bin crdgen | kubectl apply -f");
        std::process::exit(1);
    }

    // if let Err(e) = vpa_api.list(&ListParams::default().limit(1)).await {
    //     error!("vpa crd is not querable; {e:?}, is the crd intalled?");
    //     std::process::exit(1);
    // }

    let mut controller = Controller::new(gen_api.clone(), Config::default());
    let store = controller.store();

    for gvk in &gvks {
        let api_resource = ApiResource::from_gvk(gvk);
        let dyn_api: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource);
        let dyn_mapper = |store: Store<AutoVPA>| {
            move |o: DynamicObject| {
                store
                    .find(|g| match &o.metadata.labels {
                        Some(labels) => utils::match_label(&g.spec.selector, &labels),
                        None => false,
                    })
                    .map(|g| ObjectRef::from_obj(&*g))
            }
        };

        controller = controller.watches_with(
            dyn_api,
            api_resource,
            Config::default(),
            dyn_mapper(store.clone()),
        );
    }

    controller
        .owns(vpa_api.clone(), Config::default())
        .shutdown_on_signal()
        .run(reconciler, error_policy, Arc::new(Ctx { client: client.clone(), gvks }))
        .for_each(|res| async move {
            match res {
                Ok(o) => info!("reconciled: {:?}", o),
                Err(err) => error!("reconcile failed: {}", err),
            }
        })
        .await;
    Ok(())
}

async fn reconciler(obj: Arc<AutoVPA>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let client = ctx.client.clone();
    let label_selector_query = convert_label_selector_to_query_string(&obj.spec.selector)?;

    let oref = obj.controller_owner_ref(&()).ok_or(Error::InvalidOwnerRef())?;

    for gvk in &ctx.gvks {
        let api_resource = ApiResource::from_gvk(gvk);
        let dyn_api: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource);

        let targets = dyn_api
            .list(&ListParams {
                label_selector: Some(label_selector_query.clone()),
                ..Default::default()
            })
            .await?
            .items;

        for target in targets {
            let target_name = target
                .meta()
                .name
                .clone()
                .ok_or_else(|| Error::MissingObjectKey(".metadata.name"))?;

            let target_ref = VerticalPodAutoscalerTargetRef {
                api_version: Some(gvk.api_version()),
                kind: gvk.kind.clone(),
                name: target_name.clone(),
            };

            let vpa_name = format!("{}-vpa", target_name.clone());

            let target_namespace =
                target.namespace().ok_or(Error::MissingObjectKey(".metadata.namespace"))?;

            let vpa = VerticalPodAutoscaler {
                metadata: ObjectMeta {
                    name: Some(vpa_name.clone()),
                    namespace: Some(target_namespace.clone()),
                    owner_references: Some(vec![oref.clone()]),
                    ..obj.spec.vpa_template.metadata.clone().unwrap_or(Default::default())
                },
                spec: VerticalPodAutoscalerSpec {
                    target_ref: Some(target_ref),
                    ..obj.spec.vpa_template.template.clone()
                },
            };

            let vpa_api: Api<VerticalPodAutoscaler> =
                Api::namespaced(client.clone(), &target_namespace);

            info!("apply vpa: {}", serde_yaml::to_string(&vpa)?);

            match vpa_api
                .patch(&vpa_name, &PatchParams::apply("autovpa.dev"), &Patch::Apply(&vpa))
                .await
            {
                Ok(_) => (),
                Err(err) => error!("apply vpa failed: {}", err),
            };
        }
    }
    Ok(Action::await_change())
}

fn error_policy(_obj: Arc<AutoVPA>, _error: &Error, _ctx: Arc<Ctx>) -> Action {
    Action::requeue(Duration::from_secs(5))
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use futures::{StreamExt, TryStreamExt};
    use k8s_openapi::apimachinery::pkg::{api::resource::Quantity, apis::meta::v1::LabelSelector};
    use kube::{
        api::{Patch, PatchParams},
        core::watch,
        runtime::{watcher, WatchStreamExt},
        Api,
    };

    use crate::{
        vpa::{
            ContainerControlledValues::RequestsAndLimits, ContainerPolicies, VerticalPodAutoscaler,
            VerticalPodAutoscalerResourcePolicy, VerticalPodAutoscalerSpec,
        },
        AutoVPA,
    };

    #[tokio::test]
    async fn integration_test_apply_vpa() -> anyhow::Result<()> {
        let client = kube::Client::try_default().await.unwrap();
        let gen_api: Api<AutoVPA> = Api::namespaced(client.clone(), "default");
        let auto_vpa = get_test_vpa_gen();
        println!("{:?}", auto_vpa.clone());
        gen_api
            .patch("test-vpa-gen", &PatchParams::apply("autovpa.dev"), &Patch::Apply(auto_vpa))
            .await
            .unwrap();

        kube::runtime::watcher(gen_api, watcher::Config::default())
            .applied_objects()
            .try_for_each(|g| async move {
                println!("watched auto-vpa : {:?}", g);
                Ok(())
            })
            .await?;

        let vpa = get_test_vpa();
        dbg!(vpa);

        Ok(())
    }

    fn get_test_vpa_gen() -> AutoVPA {
        AutoVPA::new(
            "test-vpa-gen",
            crate::AutoVPASpec {
                selector: LabelSelector {
                    match_expressions: None,
                    match_labels: Some(std::collections::BTreeMap::from([(
                        "app".into(),
                        "nginx".into(),
                    )])),
                },
                vpa_template: crate::VerticalPodAutoscalerTemplateSpec {
                    metadata: None,
                    template: VerticalPodAutoscalerSpec {
                        recommenders: None,
                        target_ref: None,
                        // VerticalPodAutoscalerTargetRef {
                        //     // api_version: Some("apps/v1".to_owned()),
                        //     // kind: "Deployment".to_owned(),
                        //     // name: "nginx-deployment".to_owned(),
                        // },
                        resource_policy: Some(VerticalPodAutoscalerResourcePolicy {
                            container_policies: Some(vec![ContainerPolicies {
                                container_name: Some("nginx".to_string()),
                                controlled_resources: Some(vec!["cpu".into(), "memory".into()]),
                                controlled_values: Some(RequestsAndLimits),
                                max_allowed: Some(BTreeMap::from([
                                    ("cpu".into(), Quantity("2".into())),
                                    ("memory".into(), Quantity("2048Mi".into())),
                                ])),
                                min_allowed: Some(BTreeMap::from([
                                    ("cpu".into(), Quantity("1".into())),
                                    ("memory".into(), Quantity("48Mi".into())),
                                ])),
                                mode: None,
                            }]),
                        }),
                        update_policy: Some(Default::default()),
                    },
                },
            },
        )
    }

    fn get_test_vpa() -> VerticalPodAutoscaler {
        let vpa_yaml = r"
        apiVersion: autoscaling.k8s.io/v1
        kind: VerticalPodAutoscaler
        metadata:
          labels:
            app.oam.dev/cluster: default
          name: nginx-vpa
        spec:
          resourcePolicy:
            containerPolicies:
            - containerName: nginx
              controlledResources:
              - cpu
              - memory
              controlledValues: RequestsAndLimits
              maxAllowed:
                cpu: 2
                memory: 2048Mi
              minAllowed:
                cpu: 1
                memory: 48Mi
          targetRef:
            apiVersion: apps/v1
            kind: Deployment
            name: nginx-deployment
          updatePolicy:
            updateMode: Auto
        ";
        serde_yaml::from_str(vpa_yaml).expect("illegal input vpa yaml")
    }
}
