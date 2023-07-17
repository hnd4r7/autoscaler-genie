use crate::utils::{self, convert_label_selector_to_query_string};
use crate::vpa::VerticalPodAutoscalerTargetRef;
use futures::StreamExt;
use kube::api::{Patch, PatchParams};
use kube::core::{DynamicObject, GroupVersionKind};
use kube::discovery::ApiResource;
use kube::runtime::reflector::{ObjectRef, Store};
use kube::runtime::watcher::Config;
use kube::runtime::Controller;
use kube::{Api, Resource, ResourceExt};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing_subscriber::{prelude::*, EnvFilter, Registry};

use crate::vpa::{VerticalPodAutoscaler, VerticalPodAutoscalerSpec};
use k8s_openapi::{
    apimachinery::pkg::apis::meta::v1::LabelSelector, apimachinery::pkg::apis::meta::v1::ObjectMeta,
};
use kube::{api::ListParams, runtime::controller::Action, Client, CustomResource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::*;

struct Ctx {
    client: Client,
    gvks: Vec<GroupVersionKind>,
}
// Define the AutoVPA CRD struct
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[kube(group = "autovpa.dev", version = "v1", kind = "AutoVPA")]
#[kube(status = "AutoVPAStatus")]
#[kube(printcolumn = r#"{"name":"matched", "jsonPath": ".status.matched", "type": "integer"}"#)]
#[serde(rename_all = "camelCase")]
pub struct AutoVPASpec {
    namespace_selector: Option<Vec<String>>,
    object_selector: Option<LabelSelector>,
    vpa_template: VerticalPodAutoscalerTemplateSpec,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
pub struct AutoVPAStatus {
    matched: i32,
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

pub fn init_logging() {
    let logger = tracing_subscriber::fmt::layer().compact();
    let env_filter =
        EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info")).unwrap();
    let collector = Registry::default().with(logger).with(env_filter);
    // Initialize tracing
    tracing::subscriber::set_global_default(collector).unwrap();
}

pub async fn run() -> anyhow::Result<()> {
    init_logging();

    let gvks = vec![
        GroupVersionKind::gvk("apps", "v1", "Deployment"),
        GroupVersionKind::gvk("apps", "v1", "StatefulSet"),
        GroupVersionKind::gvk("apps", "v1", "DaemonSet"),
        GroupVersionKind::gvk("batch", "v1", "Job"),
    ];

    let client = Client::try_default().await.expect("failed to create kube client");

    let gen_api: Api<AutoVPA> = Api::all(client.clone());
    let vpa_api: Api<VerticalPodAutoscaler> = Api::all(client.clone());

    // In sceniro of oam controlled contrllers, there is a oam.dev.namespace label in the generated deployment | statefulset...
    if let Err(e) = gen_api.list(&ListParams::default().limit(1)).await {
        error!(
            "autovpa crd is not querable; {:?}, is the crd intalled?",
            &e as &dyn std::error::Error
        );
        info!("Installation: cargo run --bin crdgen | kubectl apply -f");
        std::process::exit(1);
    }

    if let Err(e) = vpa_api.list(&ListParams::default().limit(1)).await {
        error!("vpa crd is not querable; {:?}, is the crd intalled?", &e as &dyn std::error::Error);
        std::process::exit(1);
    }

    let mut controller = Controller::new(gen_api.clone(), Config::default());
    let store = controller.store();

    for gvk in &gvks {
        let api_resource = ApiResource::from_gvk(gvk);
        let dyn_api: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource);
        let dyn_mapper = |store: Store<AutoVPA>| {
            move |o: DynamicObject| {
                store
                    .find(|g| {
                        let match_namespace = g.spec.namespace_selector.as_ref().map_or(true, |mn| {
                                o.namespace().map_or(false, |os| mn.contains(&os))
                            });
                        if !match_namespace {
                            return false
                        }
                        // select "Nothing" when selector is none, select "Everything" when selector is empty struct.
                        // ref: https://github.com/kubernetes/kubernetes/blob/master/vendor/k8s.io/apimachinery/pkg/apis/meta/v1/helpers.go#L36
                        let match_labels = g.spec.object_selector.as_ref().map_or(false, |ml| {
                            o.metadata.labels.as_ref().map_or(true, |ls| utils::match_label(ml, ls))
                        });
                        debug!(
                            "g selector {:?} match {:?} return {:?}",
                            g.spec.object_selector, o.metadata.name, match_labels
                        );
                        match_labels
                    })
                    .map(|g|ObjectRef::from_obj(&*g))
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
                Err(err) => {
                    error!(error = &err as &dyn std::error::Error, "Failed to reconcile object")
                }
            }
        })
        .await;
    Ok(())
}

async fn reconciler(obj: Arc<AutoVPA>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let client = ctx.client.clone();
    let label_selector_query = if let Some(selector) = &obj.spec.object_selector {
        Some(convert_label_selector_to_query_string(selector)?)
    } else {
        None
    };

    let oref = obj.controller_owner_ref(&()).ok_or(Error::InvalidOwnerRef())?;

    let mut matched = 0;
    for gvk in &ctx.gvks {
        let api_resource = ApiResource::from_gvk(gvk);
        let dyn_api: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource);

        let targets = dyn_api
            .list(&ListParams {
                label_selector: label_selector_query.clone(),
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

            let target_namespace =
                target.namespace().ok_or(Error::MissingObjectKey(".metadata.namespace"))?;

            match &obj.spec.namespace_selector {
                Some(ns) => if !ns.contains(&target_namespace.clone()){
                    debug!("skip obj with namespace: {}", target_namespace.clone());
                    continue
                },
                _ => ()
            }

            let target_ref = VerticalPodAutoscalerTargetRef {
                api_version: Some(gvk.api_version()),
                kind: gvk.kind.clone(),
                name: target_name.clone(),
            };

            let vpa_name = format!("{}-vpa", target_name.clone());


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

            match vpa_api
                .patch(&vpa_name, &PatchParams::apply("autovpa.dev"), &Patch::Apply(&vpa))
                .await
            {
                Ok(_) => info!("apply vpa {} successfully", vpa_name),
                Err(err) => error!("apply vpa failed: {}", err),
            };
            matched += 1;
        }
    }

    let api: Api<AutoVPA> = Api::all(client.clone());

    let status = serde_json::json!({"status": AutoVPAStatus { matched }});
    api.patch_status(&obj.name_any(), &Default::default(), &Patch::Merge(status)).await?;

    Ok(Action::await_change())
}

fn error_policy(_obj: Arc<AutoVPA>, _error: &Error, _ctx: Arc<Ctx>) -> Action {
    Action::requeue(Duration::from_secs(5))
}

#[cfg(test)]
mod test {
    use std::{collections::BTreeMap, sync::Arc};

    use futures::TryStreamExt;
    use k8s_openapi::{
        api::apps::v1::Deployment,
        apimachinery::pkg::{api::resource::Quantity, apis::meta::v1::LabelSelector},
    };
    use kube::{
        api::{Patch, PatchParams},
        core::GroupVersionKind,
        runtime::{watcher, WatchStreamExt},
        Api, ResourceExt,
    };

    use crate::{
        controller::{reconciler, Ctx},
        vpa::{
            ContainerControlledValues::RequestsAndLimits, ContainerPolicies, VerticalPodAutoscaler,
            VerticalPodAutoscalerResourcePolicy, VerticalPodAutoscalerSpec,
        },
        AutoVPA,
    };

    #[tokio::test]
    #[ignore = "use k8s current-context"]
    async fn integration_test_apply_vpa() -> anyhow::Result<()> {
        let client = kube::Client::try_default().await.unwrap();
        let gen_api: Api<AutoVPA> = Api::all(client.clone());

        let autovpa_name = "office-test-autovpa";

        let auto_vpa = get_test_vpa_gen(autovpa_name);
        let test_workload = get_test_workload();

        let workload_api: Api<Deployment> = Api::default_namespaced(client.clone());
        // println!("{:?}", auto_vpa.clone());
        gen_api
            .patch(
                autovpa_name,
                &PatchParams::apply("autovpa.dev"),
                &Patch::Apply(auto_vpa.clone()),
            )
            .await
            .unwrap();

        workload_api
            .patch("nginx-deployment", &PatchParams::apply("autovpa.dev"), &Patch::Apply(test_workload))
            .await
            .unwrap();

        let auto_vpa = gen_api.get(autovpa_name).await?;
        reconciler(
            Arc::new(auto_vpa.clone()),
            Arc::new(Ctx {
                client: client.clone(),
                gvks: vec![GroupVersionKind::gvk("apps", "v1", "Deployment")],
            }),
        )
        .await
        .map_err(|err| {
            println!("{}", &err as &dyn std::error::Error);
        })
        .ok();

        //verify side effects happened
        let output = gen_api.get_status(&auto_vpa.name_any()).await?;
        assert_eq!(1, output.status.unwrap().matched);

        // kube::runtime::watcher(gen_api, watcher::Config::default())
        //     .applied_objects()
        //     .try_for_each(|g| async move {
        //         println!("watched auto-vpa : {:?}", g);
        //         Ok(())
        //     })
        //     .await?;

        let vpa = get_expected_vpa();
        dbg!("expected vpa:", vpa);
        Ok(())
    }

    fn get_test_vpa_gen(name: &str) -> AutoVPA {
        let test_yaml = format!(r#"
apiVersion: autovpa.dev/v1
kind: AutoVPA
metadata:
  name: {}
spec:
  namespaceSelector:
  - ali-office-test
  objectSelector:
    matchLabels:
      app: santa
  vpaTemplate:
    template:
      resourcePolicy:
        containerPolicies:
        - containerName: "*"
          controlledResources:
          - cpu
          - memory
          controlledValues: RequestsAndLimits
          maxAllowed:
            cpu: 50m
            memory: 100Mi
          minAllowed:
            cpu: "6"
            memory: 8Gi
      updatePolicy:
        updateMode: Auto
        "#, name);
        serde_yaml::from_str(&test_yaml).expect("invalid test autovpa yaml")
    }

    fn get_expected_vpa() -> VerticalPodAutoscaler {
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
                cpu: '2'
                memory: 2048Mi
              minAllowed:
                cpu: '1'
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

    fn get_test_workload() -> Deployment {
        let deployment_yaml = r#"
        apiVersion: apps/v1
        kind: Deployment
        metadata:
          name: nginx-deployment
          labels:
            app: nginx
        spec:
          replicas: 3
          selector:
            matchLabels:
              app: nginx
          template:
            metadata:
              labels:
                app: nginx
            spec:
              containers:
              - name: nginx
                image: nginx:1.14.2
                ports:
                - containerPort: 80
        "#;
        serde_yaml::from_str(deployment_yaml).expect("illegal input vpa yaml")
    }
}
