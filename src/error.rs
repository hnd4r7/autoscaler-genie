use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("LabelSelector is invalid: {0}")]
    InvalidLabelSelector(String),

    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
    },
}

type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
#[test]
fn test_err() {
    // Error::InvalidLabelSelector("xxx".into())
}
