# Autoscaler-Genie

Autoscaler-Genie is a tool that automates the creation of Vertical Pod Autoscaler (VPA) resources for your Kubernetes workloads. The Vertical Pod Autoscaler is a Kubernetes component that automatically adjusts the resource requests and limits of your containers based on their actual resource usage.

With Autoscaler-Genie, you can simplify the process of creating and managing VPAs by automating the generation and application of VPA configurations for your workloads.

## Features

- Automatically generates VPA resources for your Kubernetes workloads.
- Adjusts resource requests and limits based on container resource usage.
- Simplifies the management of VPAs by automating the creation process.

## Getting Started

### Prerequisites

To use Autoscaler-Genie, you need:

- Kubernetes cluster (version X.X or higher)
- Kubernetes configuration (Kubeconfig) set up to access your cluster

### Installation

1. Clone the Autoscaler-Genie repository:

   ```shell
   git clone https://github.com/your-username/autoscaler-genie.git
   ```
2. Build the Autoscaler-Genie binary:
```
cd autoscaler-genie
make build
```

### Usage

Set up the necessary RBAC permissions for Autoscaler-Genie to access and modify resources in your cluster. Example YAML manifests for RBAC are provided in the rbac directory.
```
kubectl apply -f rbac/autoscaler-genie-rbac.yaml
```
Create a configuration file describing your workloads and their desired VPA settings. Example configuration files are provided in the examples directory.

```shell
cp examples/workload-config.yaml my-workload-config.yaml
```
Edit my-workload-config.yaml to specify your workload details and desired VPA settings.
Run Autoscaler-Genie to generate and apply VPAs for your workloads:
```shell
./autoscaler-genie --config my-workload-config.yaml
```
Autoscaler-Genie will read your workload configuration file, generate the necessary VPA resources, and apply them to your cluster.

### Contributing
Contributions to Autoscaler-Genie are welcome! If you find a bug, have a feature request, or want to contribute code, please follow our contribution guidelines outlined in the CONTRIBUTING.md file.

### License
Autoscaler-Genie is released under the MIT License.
Feel free to customize the README template to provide more specific information about your Autoscaler