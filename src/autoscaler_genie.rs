use crate::error::Error;
use crate::utils::convert_label_selector_to_query_string;
use futures::StreamExt;
use k8s_openapi::ClusterResourceScope;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::{
    utils,
    vpa::{VerticalPodAutoscaler, VerticalPodAutoscalerSpec},
};
use k8s_openapi::{
    api::apps::v1::{DaemonSet, Deployment, StatefulSet},
    apimachinery::pkg::apis::meta::v1::LabelSelector,
    apimachinery::pkg::apis::meta::v1::ObjectMeta,
    Metadata, Resource,
};
use kube::{
    api::{Api, ListParams, PostParams},
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
}

// Define the AutoScalerGenie CRD struct
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[kube(group = "autoscalegenie.dev", version = "v1", kind = "AutoScalerGenie", namespaced)]
struct AutoScalerGenieSpec {
    selector: LabelSelector,
    vpa_template: VerticalPodAutoscalerTemplateSpec,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
struct VerticalPodAutoscalerTemplateSpec {
    /// Standard object's metadata. More info: https://git.k8s.io/community/contributors/devel/sig-architecture/api-conventions.md#metadata
    metadata: Option<ObjectMeta>,
    template: VerticalPodAutoscalerSpec,
}

// enum WatchApi {
//     deployment(Api<Deployment>),
// }

// Define the main function
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Client::try_default().await.expect("failed to create kube client");
    //TODO: use namespaced api?
    let gen_api: Api<AutoScalerGenie> = Api::all(client.clone());

    // In sceniro of oam controlled contrllers, there is a oam.dev.namespace label in the generated deployment | statefulset...
    if let Err(e) = gen_api.list(&ListParams::default().limit(1)).await {
        error!("autoscalergenie crd is not querable; {e:?}, is the crd intalled?");
        info!("Installation: cargo run --bin crdgen | kubectl apply -f");
        std::process::exit(1);
    }

    let controller = Controller::new(gen_api, Config::default());

    let deployment_api: Api<Deployment> = Api::all(client.clone());
    let statefulset_api: Api<StatefulSet> = Api::all(client.clone());
    let daemonset_api: Api<DaemonSet> = Api::all(client.clone());

    let vpa_api: Api<VerticalPodAutoscaler> = Api::all(client.clone());
    let store = controller.store();

    let deployment_mapper = |store: Store<AutoScalerGenie>| {
        move |o: Deployment| {
            // move |o: &dyn Metadata<Ty = ObjectMeta, Scope = ClusterResourceScope>| {
            store
                .find(|g| match &o.metadata().labels {
                    Some(labels) => utils::match_label(&g.spec.selector, &labels),
                    None => false,
                })
                .map(|g| ObjectRef::from_obj(&*g))
        }
    };
    let statefulset_mapper = |store: Store<AutoScalerGenie>| {
        move |o: StatefulSet| {
            store
                .find(|g| match &o.metadata().labels {
                    Some(labels) => utils::match_label(&g.spec.selector, &labels),
                    None => false,
                })
                .map(|g| ObjectRef::from_obj(&*g))
        }
    };
    let daemonset_mapper = |store: Store<AutoScalerGenie>| {
        move |o: DaemonSet| {
            store
                .find(|g| match &o.metadata().labels {
                    Some(labels) => utils::match_label(&g.spec.selector, &labels),
                    None => false,
                })
                .map(|g| ObjectRef::from_obj(&*g))
        }
    };

    controller
        .watches(deployment_api, Config::default(), deployment_mapper(store.clone()))
        .watches(statefulset_api, Config::default(), statefulset_mapper(store.clone()))
        .watches(daemonset_api, Config::default(), daemonset_mapper(store.clone()))
        .owns(vpa_api, Config::default())
        .shutdown_on_signal()
        .run(reconciler, error_policy, Arc::new(Ctx { client: client.clone() }))
        //TODO err handling
        .for_each(|_| futures::future::ready(()))
        .await;
    Ok(())
}

async fn reconciler(obj: Arc<AutoScalerGenie>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let client = ctx.client.clone();
    // let gen_api: Api<VerticalPodAutoscaler> = Api::all(client);
    let label_selector_query = convert_label_selector_to_query_string(&obj.spec.selector)?;

    // calculate difference of current vpas and corespond obj
    let deployment_api: Api<Deployment> = Api::all(client.clone());

    let match_deployments = deployment_api
        .list(&ListParams { label_selector: Some(label_selector_query), ..Default::default() }).await?;

    let statefulset_api: Api<StatefulSet> = Api::all(client.clone());
    let daemonset_api: Api<DaemonSet> = Api::all(client.clone());

    // If ref obj is removed, vpa should be removed too.
    Ok(Action::await_change())
}

fn error_policy(_obj: Arc<AutoScalerGenie>, _error: &Error, _ctx: Arc<Ctx>) -> Action {
    Action::requeue(Duration::from_secs(5))
}

/*
    // Define the list of labels to match
    let labels = [("app", "example"), ("tier", "backend")];

    loop {
        // List all AutoScalerGenie objects in the default namespace
        let autoscalegenie_list = autoscalegenie_api.list(&ListParams::default()).await?.items;

        // Iterate over each AutoScalerGenie object
        for autoscalegenie in autoscalegenie_list {
            // Check if the AutoScalerGenie object matches the labels
            let metadata = autoscalegenie.metadata.clone();
            let object_labels = metadata.labels.unwrap_or_default();
            if labels.iter().all(|(k, v)| object_labels.get(*k) == Some(v)) {
                // If the AutoScalerGenie object matches the labels, create a VPA for each Deployment and StatefulSet in the namespace
                let namespace = autoscalegenie.spec.namespace;
                let deployments_api: Api<Deployment> = Api::namespaced(client.clone(), &namespace);
                let statefulsets_api: Api<StatefulSet> =
                    Api::namespaced(client.clone(), &namespace);
                let deployments = deployments_api.list(&ListParams::default()).await?.items;
                let statefulsets = statefulsets_api.list(&ListParams::default()).await?.items;
                for deployment in deployments {
                    let deployment_name = deployment.metadata.name.clone();
                    let deployment_labels = deployment.metadata.labels.clone().unwrap_or_default();
                    if labels.iter().all(|(k, v)| deployment_labels.get(*k) == Some(v)) {
                        // If the Deployment matches the labels, create a VPA for it
                        let vpa = serde_json::json!({
                            "apiVersion": "autoscaling.k8s.io/v1",
                            "kind": "VerticalPodAutoscaler",
                            "metadata": {
                                "name": format!("{}-vpa", deployment_name),
                                "namespace": namespace,
                            },
                            "spec": {
                                "targetRef": {
                                    "apiVersion": "apps/v1",
                                    "kind": "Deployment",
                                    "name": deployment_name,
                                },
                                "updatePolicy": {
                                    "updateMode": "Auto",
                                },
                                "resourcePolicy": {
                                    "containerPolicies": [
                                        {
                                            "containerName": "app",
                                            "mode": "Auto",
                                            "minAllowed": {
                                                "cpu": autoscalegenie.spec.min_cpu.clone(),
                                                "memory": autoscalegenie.spec.min_memory.clone(),
                                            },
                                            "maxAllowed": {
                                                "cpu": autoscalegenie.spec.max_cpu.clone(),
                                                "memory": autoscalegenie.spec.max_memory.clone(),
                                            },
                                        },
                                    ],
                                },
                            },
                        });
                        let vpas_api: Api<VerticalPodAutoscaler> =
                            Api::namespaced(client.clone(), &namespace);
                        let pp = PostParams::default();
                        vpas_api.create(&pp, &vpa).await?;
                    }
                }
                for statefulset in statefulsets {
                    let statefulset_name = statefulset.metadata.name.clone();
                    let statefulset_labels =
                        statefulset.metadata.labels.clone().unwrap_or_default();
                    if labels.iter().all(|(k, v)| statefulset_labels.get(*k) == Some(v)) {
                        // If the StatefulSet matches the labels, create a VPA for it
                        let vpa = serde_json::json!({
                            "apiVersion": "autoscaling.k8s.io/v1",
                            "kind": "VerticalPodAutoscaler",
                            "metadata": {
                                "name": format!("{}-vpa", statefulset_name),
                                "namespace": namespace,
                            },
                            "spec": {
                                "targetRef": {
                                    "apiVersion": "apps/v1",
                                    "kind": "StatefulSet",
                                    "name": statefulset_name,
                                },
                                "updatePolicy": {
                                    "updateMode": "Auto",
                                },
                                "resourcePolicy": {
                                    "containerPolicies": [
                                        {
                                            "containerName": "app",
                                            "mode": "Auto",
                                            "minAllowed": {
                                                "cpu": autoscalegenie.spec.min_cpu.clone(),
                                                "memory": autoscalegenie.spec.min_memory.clone(),
                                            },
                                            "maxAllowed": {
                                                "cpu": autoscalegenie.spec.max_cpu.clone(),
                                                "memory": autoscalegenie.spec.max_memory.clone(),
                                            },
                                        },
                                    ],
                                },
                            },
                        });
                        let vpas_api: Api<VerticalPodAutoscaler> =
                            Api::namespaced(client.clone(), &namespace);
                        let pp = PostParams::default();
                        vpas_api.create(&pp, &vpa).await?;
                    }
                }
            }
        }
        // Wait for 10 seconds before checking again
        tokio::time::delay_for(std::time::Duration::from_secs(10)).await;
    }
*/
