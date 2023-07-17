# Autoscaler-Genie

Autoscaler-Genie is a tool that automates the creation of Vertical Pod Autoscaler (VPA) resources for your Kubernetes workloads. The Vertical Pod Autoscaler is a Kubernetes component that automatically adjusts the resource requests and limits of your containers based on their actual resource usage.

With Autoscaler-Genie, you can simplify the process of creating and managing VPAs by automating the generation and application of VPA configurations for your workloads.

## Features

- Automatically generates VPA resources for your Kubernetes workloads.
- Simplifies the management of VPAs by automating the creation process.


## Prerequisites

To use Autoscaler-Genie, you need:

- Kubernetes cluster
- Kubernetes configuration (Kubeconfig) set up to access your cluster

## Installation

1. Clone the Autoscaler-Genie repository:

   ```shell
   git clone https://github.com/your-username/autoscaler-genie.git
   ```

2. Build the Autoscaler-Genie binary:

```
cd autoscaler-genie
just generate
```
3. apply crd and necessary RBAC permission YAML manifests to your cluster
```
just apply
```

### Usage

Autoscaler-genie generates Vertical Pod Autoscaler (VPA)  for your Kubernetes workloads based on specified selectors. It now supports matching workloads by label or by namespace. You can specify the VPA template you want. 

When applied, it will generate the necessary VPA resources and apply them to your cluster. The AutoVpa CRD will also display the total number of matched workloads and the generated VPAs

For example:
```yaml
apiVersion: autovpa.dev/v1
kind: AutoVPA
metadata:
  name: test-vpa-gen
spec:
  namespaceSelector:
  - kube-system
  objectSelector:
    matchLabels:
      app: nginx
  vpa_template:
    metadata: null
    template:
      resourcePolicy:
        containerPolicies:
        - containerName: "*"
          controlledResources:
          - cpu
          - memory
          controlledValues: RequestsAndLimits
          maxAllowed:
            cpu: "2"
            memory: 2048Mi
          minAllowed:
            cpu: "1"
            memory: 48Mi
      updatePolicy:
        updateMode: Auto
```

This will generate vpa for all the workload(`Deployment\StatefulSet\Daemonset\Job`) with label `app=nginx` within the `kube-system` namespace

### Contributing
Contributions to Autoscaler-Genie are welcome! If you find a bug, have a feature request, or want to contribute code, please follow our contribution guidelines outlined in the CONTRIBUTING.md file.

### License
Autoscaler-Genie is released under the MIT License.