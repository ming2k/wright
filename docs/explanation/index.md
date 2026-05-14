# Explanation

Discussions that explain why things work the way they do.

- [Architecture](architecture.md)
- [Launch Design](launch-design.md) — Mission, convergence, root isolation, and the provisioning pipeline
- [Execution Hierarchy](execution-hierarchy.md)
- [Checkpoint Recovery](checkpoint-recovery.md)
- [Plan Build](plan-build.md) — How plans are discovered, built, sealed, and deployed
- [Batch Processing](batch-processing.md) — How tasks are grouped into waves and why batches are sealed and deployed as a unit
- [Dependency DAG](dependency-dag.md) — How Wright uses two separate DAGs for build ordering and runtime installation ordering
- [Delivery Recovery](delivery-recovery.md)
- [Isolation Model](isolation-model.md)
- [OverlayFS Layers](overlayfs-layers.md) — How OverlayFS is used in the build pipeline and isolation sandbox, and the busy races it creates
- [Filesystem Hierarchy Standard](filesystem-hierarchy-standard.md)
