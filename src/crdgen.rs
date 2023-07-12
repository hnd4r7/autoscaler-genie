use autoscaler_genie::AutoVPA;
use kube::CustomResourceExt;

fn main() {
    print!("{}", serde_yaml::to_string(&AutoVPA::crd()).unwrap())
}
