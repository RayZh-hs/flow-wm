# First upgrade from the embedded runtime

The supported live runtime reads only native format 3. A pre-refactor installation
needs an explicitly invoked, one-time conversion and coordinated service startup.
Normal load/save and compatible `install.py --replace` never import old state.

Build a complete matching release first:

```sh
python3 scripts/dev.py build all --release
```

From the running installed FlowWM compositor environment, prepare the upgrade:

```sh
python3 scripts/upgrade_legacy.py --prepare
```

Preparation leaves services, configuration, selected bundle and original data
running as they are. It creates a private directory under
`$XDG_STATE_HOME/flow-wm/upgrades/`, preserves configuration, saved documents,
service files and launchers, and stages a complete bundle. It validates a copied
profile with the new compositor and writes a preview native import through an
isolated production host. All imported workflows remain unassigned in the
validation host: no imported jobs or sources execute during conversion.

Inspect the printed directory's `upgrade.json`, `preview/import-report.json` and
converted documents, then explicitly activate that directory:

```sh
python3 scripts/upgrade_legacy.py --apply /path/printed/by/prepare
```

Application checks that the compositor/session still matches preparation, stops
the old GTK/DMS/observer services, preserves a final saved v1 document, and converts
that final copy. It publishes current configuration/manifests and separate native
runtime, presentation and desktop documents. The original `state.json` remains
untouched. It installs the required host/provider units, selects the staged bundle
and starts host, desktop provider, GTK, DMS and observer together. Readiness checks
require current protocol replies, a current catalog, preserved workflow IDs,
active services and unchanged compositor/window identities. The journal records
the result and backup location. A failure does not restart the compositor or
silently choose an old embedded runtime; inspect the journal for forward recovery.

A compositor binary update still takes effect at the next login. Interactive CLI
commands select the current installed bundle even when an existing terminal or
compositor inherited the previous `FLOWWM_INSTALL_ROOT`. Long-running services
retain their explicit bundle pin, allowing later compatible GTK/DMS replacement
to preserve the independent host/provider.

## Conversion boundary

The separate `scripts/import_legacy.py` accepts standalone v1 state and writes to
a **new directory**. IDs, supported node definitions, typed connections, recipes,
layout and notes are preserved. Single-parent dependencies become explicit signals;
gate/human success becomes `accepted`, failure becomes `rejected`. Multiple
success parents become an explicit correlated Join, including terminal failure
inputs. The current core validates the resulting IDs, schemas, types and edges.

Imported runtime records contain no executions, process IDs, native window IDs,
application registrations or replayed outputs. The desktop provider rediscovers
windows using declarative app/title selectors and historical row preferences,
and creates fresh session-scoped registrations. Source input and discarded
execution status remain in the backup and import report; no execution continuity
is fabricated.

Unsupported watcher side tables, implicit review flags, repeating Metric
compatibility and all-parent failure policies abort conversion with an explanation.
Author those cases explicitly as current sources/decisions/branches before
importing. Active owned jobs must be stopped or resolved before applying the first
upgrade. Observed running application windows are preserved separately.

Declarative manifest versions 1/2 with known fields can be upgraded in the staged
profile to version 3 without executing their source. Unknown fields/formats and
old runtime JSON configuration require explicit handling rather than guessed
conversion. Normal runtime/manifest loaders still accept only current versions.

For importing a preserved file independently of service installation:

```sh
python3 scripts/import_legacy.py /backup/state.json /new/native-output \
    --host target/release/flow-core \
    --manifest /current/extensions/example.json
```

After a successful first upgrade, use the normal
`python3 scripts/install.py --skip-build --replace` for compatible release bundles.
Pre-refactor bundles remain unsupported by normal rollback; retain the first
upgrade's preserved originals separately.
