# cercis-core (kernel)

**English** | [中文](README.zh-CN.md)

The Cercis kernel: a **domain-neutral "folder-is-a-database" engine**.

- It does exactly two things: **data storage** (tables, records, files, history, query
  primitives) and **AI-friendliness** (data contract, machine manifest, JSON, permissions).
- It knows no discipline-specific concepts; field semantics come from kit templates.
- Written in Rust; the goal is a single binary, every platform, fully offline.

## Status (the kernel is complete)

- [x] Project creation and opening (`project.yaml` + directory skeleton + AGENTS.md)
- [x] Table read/write (CSV: UTF-8 without BOM, LF, lowercase ASCII underscore column names)
- [x] Atomic writes (temp file + rename; a power cut never leaves a half-written file); single-row
  append and **batch append** (one transaction, one log entry)
- [x] Per-change snapshots (content-addressed object store) + checkpoints (file-level backups,
  keep the latest 50 by default)
- [x] Change log (who / device / time / action / object / before-and-after fingerprints)
- [x] Corrections (the original value is never overwritten)
- [x] Templates (table structure and field semantics come from the template; no discipline words
  in the kernel)
- [x] Query primitives (combined filters + sorting + paging, reproducible results)
- [x] Referential integrity and field validation
- [x] Value types and validation (seven value domains + constraints; inference only advises,
  it never edits a template by itself)
- [x] Machine manifest + AI contract (`AGENTS.md` / `CLAUDE.md` / `manifest.json` /
  `ai_contract.json`)
- [x] **Agent skill** (`_labnote/skill/cercis/`): an operating manual for Codex / Claude Code,
  generated from the same contract; `--generic` works before any project exists
- [x] Plugin protocol (manifest, permissions, UI extension points, trust bound to a content
  fingerprint); marketplace (presets / your own, install / remove / restore)
- [x] **Dynamic composability**: plugin registry with reverse-order rollback, declarative
  prerequisites, project state fingerprint, key namespaces and conflict checks
- [x] **Permission downgrades**: the table scope / read-only / domain allow-list declared in a
  plugin manifest are actually enforced; denied writes go to the `denied` log
- [x] **Encrypted sharing with per-member access** (S11): zones are ciphertext on disk, members
  unlock with a passphrase, keys are wrapped per member, revoking a member re-encrypts, snapshots
  are encrypted too, exports are logged (`cercis share …`)
- [x] **Pack, merge and share guard** (S12): packing (full / incremental / split volumes, zones
  stay encrypted), union merge (conflicting files keep both copies, the local side is never
  overwritten, validation runs first), share guard (allow-list + synthetic markers + privacy scan)
- [x] **NAS and working copies** (FR-N01–N07): register a network location and run its health
  check, write leases, credentials in the system keychain, offline recording in a working copy and
  two-way sync (reusing pack/merge), sync status panel, cloud-drive/Git coexistence check
- [x] **Accounts and roles** (FR-N08/N09/N11): accounts store only salted hashes, open mode
  without the file, three roles (viewer / editor / admin) enforced **in the kernel**, denied writes
  logged, `plugins.admin_only_edit` actually enforced
- [x] **Diff** (FR-M05): record level (old value → effective value, with the correction reason) and
  snapshot level (line-by-line LCS)
- [x] Command line (`cercis`, with `--json`)

## Build and test

```bash
cd kernel
cargo test          # 193 tests: unit + end-to-end/integration (encryption, pack/merge, NAS,
                    # working copies, accounts and roles, diff)
cargo build         # builds the library and the command line
cargo run -p cercis-cli -- --help
```

> Status (2026-10-10): builds clean, all tests green, zero warnings; the Linux and Windows targets
> also pass `cargo check` with zero warnings.

## Command cheat sheet

```bash
cercis init <dir> --name <project name>       # create a project
cercis install-template <dir> --name example-study
cercis append <dir> --table records --set record_id=R-1 --set date=2026-03-15
cercis append-many <dir> --table records --from data.csv --source-ref imports/data.csv
cercis append-many <dir> --table records --from data.csv --dry-run   # validate only
cercis query <dir> --table records --filter value_num gt 10 --sort date:desc
cercis validate <dir>                         # referential integrity + field rules
cercis correct <dir> --table records --target R-1 --field value_num --old 1 --new 2 --reason typo
cercis log <dir> --limit 20                   # who changed what, on which device, when
cercis note <dir> --action export --summary "exported 3 rows to /tmp/x.csv"
cercis agents <dir>                           # refresh AGENTS.md / contract / manifest / skill
cercis skill <dir> --install codex            # install the agent skill into ~/.codex/skills/
cercis skill --generic --install codex        # generic skill, before you have a project
cercis diff <dir> --table records --id R-1    # per-record diff (old value → effective value)
cercis value-types / infer-types / set-field-type <dir>   # value types: list / infer / set
cercis market list                            # plugin marketplace (install / remove / restore)
cercis plugins <dir> / cercis trust <dir> --id <plugin id>

# Encrypted sharing (S11): files in a zone are ciphertext on disk, transparent once unlocked
cercis share member-add <dir> --id alice --name Alice
cercis share zone-create <dir> --name secret --path 'methods/recipes/**' --member alice:admin
cercis share unlock <dir> --member alice      # session lives in your home dir, mode 0600, never in the project
cercis share status <dir>                     # zones, key fingerprints, leftover plaintext, unreadable blobs
cercis share export <dir> --zone secret       # decrypt into exports/ for R / AI (logged)
cercis share revoke <dir> --zone secret --member bob --as alice   # remove a member: rekey + re-encrypt
cercis share lock <dir>                       # lock (--forget also drops the keychain passphrase)

# Pack and merge (S12): carry the project away, merge back as a union; the guard keeps releases clean
cercis pack <dir> --out /tmp/my-pack --part-size-mb 20            # split volumes, e.g. for chat apps
cercis pack <dir> --out /tmp/inc --since /tmp/my-pack/pack.json   # incremental: only changed files
cercis merge /tmp/my-pack --into <project on the other device>    # union merge; conflicts keep both copies
cercis guard . --json                                            # scan before sharing: allow-list / real data / privacy
cercis guard . --pack /tmp/release-pack                           # build a share package; refuses on any rejection

# NAS / network locations and working copies (FR-N01–N07): the kernel only knows mounted paths
cercis remote add <dir> --name nas --path /Volumes/lab-nas --kind smb   # register (no password)
cercis remote test <dir> --name nas                                 # health check: writable, atomic rename, case…
cercis remote credential set <dir> --name nas --password-env NAS_PW  # credentials to the keychain (never in the project)
cercis remote claim <dir> --who me@laptop --minutes 30                # write lease so other devices can see it
cercis workcopy create /Volumes/lab-nas/trial --to ~/trial-workcopy   # working copy (records offline)
cercis workcopy status ~/trial-workcopy                               # new changes on each side, last sync, conflicts
cercis workcopy sync ~/trial-workcopy                                 # two-way sync (= pack + merge); offline is reported

# Coexistence check (FR-I06): using a cloud drive or Git directory without forking
cercis sync <dir>

# Accounts and roles (FR-N08/N09/N11): salted hashes only; no account file means open mode
cercis account add <dir> --id alice --name Alice --role admin --password-env PW
cercis account login <dir> --id alice --password-env PW      # session in your home dir, not in the project
cercis account whoami <dir> / cercis account logout <dir>
cercis account role <dir> --id bob --role editor             # admin only; the last admin cannot be demoted
# Role gates live in the kernel: viewers read, editors write data, admins change structure,
# plugins, zones and accounts; denied writes are logged.
```

> Encryption details (algorithms, file format, key generations, snapshot migration) are in the
> internal requirement document §12.5.1 / D47–D49; in one sentence: **having the folder does not
> mean having the data**, and a forgotten passphrase cannot be recovered.

> Batch append is the **write primitive for imports**: the whole batch is written or nothing is,
> and it produces a single log entry (with `batch_id` and `source_ref`). Mapping, preview and
> reports belong to the basic kit.

## Hard rules

1. Data is append-only (corrections go to `corrections.csv`; originals stay forever in
   `history/objects/`)
2. Every write is atomic
3. Without this tool, the CSV and media files are still fully understandable and usable
