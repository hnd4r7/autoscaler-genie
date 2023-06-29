use kube::CustomResourceExt;
use autoscaler_genie::AutoVPA;

fn main() {
    print!("{}", serde_yaml::to_string(&AutoVPA::crd()).unwrap())
}
