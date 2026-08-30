# AGENTS.md (.github)

Ergänzt die [`AGENTS.md`](../AGENTS.md) im Repo-Root: Dort steht, was man
über *das Projekt* wissen muss, um es zu ändern. Hier steht, was sich
ausschließlich auf *GitHub als Plattform* bezieht - Branches, Pull
Requests, Workflows, Releases. Nichts hier betrifft den Rust-Code selbst.

## Branches & Merges

- Entwickelt wird ausschließlich auf `dev`. Eine automatisch von einer
  Claude-Code-Session erzeugte Branch (`claude/<slug>`) ist kein
  Dauerzustand: Arbeit von dort gehört nach `dev`, die Session-Branch wird
  anschließend nicht weitergeführt.
- `main` wird **nie** direkt gepusht. Jede Änderung an `main` läuft über
  einen Pull Request von `dev` - auch scheinbar triviale Änderungen wie
  Doku oder CI-Konfiguration.

## Pull Requests

- Vorlage: [`pull_request_template.md`](pull_request_template.md). Deckt
  sie eine Änderung nicht sauber ab, von Hand anpassen statt sie zu
  ignorieren.
- Ob ein PR einen Versions-Bump in `Cargo.toml` enthält oder nicht, muss
  aus der Beschreibung hervorgehen - das entscheidet, ob der Merge einen
  Release auslöst (siehe unten).

## CI

Zwei Workflows, eine Abhängigkeit:

- **`test.yml`**: `cargo test`, `cargo clippy --all-targets -- -D
  warnings`, `cargo fmt --check` auf einem Ubuntu-Runner. Läuft
  eigenständig auf jedem Pull Request nach `main` und ist zusätzlich als
  `workflow_call` in `build-macos.yml` eingebunden.
- **`build-macos.yml`**: Der `build`-Job (macOS-Runner, zehnfach teurer)
  hängt per `needs: [test, plan]` technisch von `test.yml` ab - nicht nur
  der Konvention nach. Ohne grüne Tests kein Build, unabhängig davon, ob
  der Workflow durch Push nach `main`, einen Tag, ein veröffentlichtes
  Release oder `workflow_dispatch` ausgelöst wurde.

## Release

Die Version in `Cargo.toml` ist die einzige Quelle. Sie erhöhen und der
Merge-PR nach `main` landet, erzeugt Tag, Release und ZIP (inklusive
`CHANGELOG.md`); Details im [Release-Abschnitt der README](../README.md#release).
Ein Tag, der nicht zu `Cargo.toml` passt, bricht den Workflow ab. Kein
Versions-Bump heißt kein Release - reine Doku-/CI-Änderungen können ohne
Versionsanhebung gemerged werden.
