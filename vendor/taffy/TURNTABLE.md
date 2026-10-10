# turntable's Taffy patch

Based on Taffy 0.13.0 (MIT). Published sources, examples, manifest and README are retained.

`TaffyTree::clear` and `TaffyTree::remove` release their removed nodes' measurement contexts as well as the primary node storage. The sparse secondary context map otherwise keeps closures for removed keys until those indices receive another context. GPUI text measurement closures capture text layouts, shaped lines, styles and text runs, so an old page could remain retained after its layout nodes were cleared.

The patch does not change layout algorithms, node IDs, primary slot-map generations, buffer capacities or rendering quality. Regression tests cover immediate context destruction, clearing repeatedly, stale-key invalidation and removing one node without dropping another node's context.

Like the vendored librespot-audio package, this copy is a separate Cargo workspace so its upstream and lifetime-regression tests can run without adding dependency packages to the app workspace:

```sh
nice -n 19 env CARGO_BUILD_JOBS=2 cargo test --manifest-path vendor/taffy/Cargo.toml --lib --all-features -- --test-threads=2
```
