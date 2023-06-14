use kube::api::{AdmissionRequest, AdmissionResponse, Resource};
use kube::error::ErrorResponse;
use kube::ResourceExt;
use serde_json::{json, Value};

// replicate match labels check. multiple vpa-genie can not have the same match label.

fn validate_autoscaler_genie(req: &AdmissionRequest) -> Result<(), ErrorResponse> {
    let resource: Resource = req.resource.clone().into();
    if resource != Resource::from_api_version_kind("autoscalergenie.example.com/v1alpha1", "AutoScalerGenie") {
        return Err(ErrorResponse {
            status: "Failure".to_string(),
            message: Some(format!(
                "Expected resource of type 'autoscalergenie.example.com/v1alpha1', got '{}'",
                resource
            )),
            ..Default::default()
        });
    }

    let obj: Value = req.object.clone().unwrap_or_default().into();
    let spec = obj["spec"].as_object().ok_or_else(|| {
        ErrorResponse::from_error("Invalid object", "Object does not have a 'spec' field")
    })?;

    let min_replicas = spec["minReplicas"].as_u64().ok_or_else(|| {
        ErrorResponse::from_error("Invalid object", "'minReplicas' field is missing or not an integer")
    })?;

    let max_replicas = spec["maxReplicas"].as_u64().ok_or_else(|| {
        ErrorResponse::from_error("Invalid object", "'maxReplicas' field is missing or not an integer")
    })?;

    if min_replicas > max_replicas {
        return Err(ErrorResponse {
            status: "Failure".to_string(),
            message: Some("'minReplicas' cannot be greater than 'maxReplicas'".to_string()),
            ..Default::default()
        });
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (kubeconfig, namespace) = kube::config::incluster_config()?;
    let client = kube::Client::new(kubeconfig, namespace);
    let server = warp::serve(warp::post().and(warp::path("validate")).map(|req: AdmissionRequest| {
        let response = validate_autoscaler_genie(&req)
            .map(|_| AdmissionResponse {
                uid: req.uid.clone(),
                allowed: true,
                ..Default::default()
            })
            .unwrap_or_else(|err| AdmissionResponse {
                uid: req.uid.clone(),
                allowed: false,
                status: Some(err),
                ..Default::default()
            });
        let body = json!({ "response": response });
        warp::reply::json(&body)
    }));
    let address = "127.0.0.1:8080";
    println!("Listening on http://{}", address);
    server.run(([127, 0, 0, 1], 8080)).await;
    Ok(())
}
