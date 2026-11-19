# ccusage-adapter-common

Share adapter file discovery, parsing infrastructure, and reports here. Choose
`EntrySource` for a loader producing standard entries and `UsageSource` for a
source with its own report pipeline. Keep typed read errors up to the shared
diagnostics boundary and keep raw log values out of warnings.

## Build layer

Built in the `foundation` Crane layer. A change here recompiles every adapter.
