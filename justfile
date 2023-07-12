[private]
default:
  @just --list --unsorted

# install crd into the cluster
install-crd: generate
  kubectl apply -f yaml/crd.yaml

generate:
  cargo run --bin crdgen > yaml/crd.yaml
  helm template charts/autoscaler-genie > yaml/deployment.yaml
# run without opentelemetry
run:
  RUST_LOG=info,kube=debug,autoscaler-genie=debug cargo run

# format with nightly rustfmt
fmt:
  cargo +nightly fmt

# run unit tests
test-unit:
  cargo test
# run integration tests
test-integration: install-crd
  cargo test -- --ignored

# compile for musl (for docker image)
compile features="":
  #!/usr/bin/env bash
  docker run --rm \
    -v $HOME/.cargo/registry/:/root/.cargo/registry \
    -v $HOME/.cargo/git:/root/.cargo/git \
    -v $PWD:/volume \
    -w /volume \
    -t clux/muslrust:stable \
    cargo build --release --features={{features}} --bin autoscaler-genie
  cp target/x86_64-unknown-linux-musl/release/autoscaler-genie .

[private]
_build features="":
  just compile {{features}}
  docker build -t hnd4r7/autoscaler-genie:local .

# docker build base
build-base: (_build "")