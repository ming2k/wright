# How to Build an Assembly

Assemblies are non-dependent, combinatory groupings of plans — items are independent units bundled for convenience, not a dependency chain. Build ordering comes from each plan's own dependency graph, not from assembly membership. Multiple assemblies can be freely combined and overlapping plans are deduplicated.

## Build an Assembly

```bash
wright build @base         # build all plans in the "base" assembly
wright build @base @devel mypackage # combine assemblies and individual plans
wright apply @base         # plan-driven install/upgrade combo
wright resolve @base --deps --match=all | wright build # override default policy
```
