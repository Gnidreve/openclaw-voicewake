# Roadmap

Nur konkret geplante, noch **nicht** umgesetzte Punkte - aufgebaut wie ein
Stack: ganz oben steht, was als Nächstes drankommt, neue Punkte werden
unten angehängt. Ein Punkt kommt aus [`ideas.md`](ideas.md) hierher, sobald
er konkret geplant ist (dort wird er dann gelöscht), und verschwindet von
hier, sobald er umgesetzt und veröffentlicht ist (dann steht er in
[`CHANGELOG.md`](CHANGELOG.md), hier wird er gelöscht). Keine Dopplung:
unkonkret in `ideas.md`, geplant hier, erledigt im Changelog - nie an zwei
Stellen gleichzeitig. Details zum Ablauf: [`AGENTS.md`](AGENTS.md#ideen-roadmap-changelog).

Die Versionsnummern sind eine **Arbeitsreihenfolge, keine feste Zusage**:
Priorität und Reihenfolge können sich verschieben, einzelne Schritte können
zusammengelegt oder aufgeteilt werden. Ungeplante Bugfixes belegen
unabhängig davon die jeweils nächste freie Patch-Version - die
Nummerierung hier verschiebt sich entsprechend nach hinten, sobald das
feststeht.

## 0.1.x - Laufende Härtung (aktuelle CLI-RPC-Architektur)

Aktuell keine offenen Punkte - neue Härtungsarbeit landet bei Bedarf
weiterhin hier, solange die aktuelle CLI-RPC-Architektur (vor 0.2.x) läuft.

## 0.2.x - WebSocket-Streaming, CLI bleibt als Legacy-Pfad

Transport-Wahl **ausschließlich über Config**, kein CLI-Flag dafür:

```toml
[openclaw]
transport = "cli"  # oder "websocket" - Default "cli", bestehende Configs laufen unverändert weiter
```

Jeder Schritt ab hier bekommt seine Tests direkt beim Bauen (wie bisher:
Unit-Tests in der Datei, ein Mock-Gateway-Feature-Test in `tests/` analog
zu `tests/pipeline_with_stubs.rs`) - nicht erst gesammelt danach in 0.3.x.
0.3.x ist für die große strukturelle Aufräumung reserviert, siehe dort.

### 0.2.6 - Sprachausgabe über das OpenClaw-Gateway (Piper-Ersatz)

Eigene Recherche nötig, bevor das begonnen wird: `Gateway-Transcription.md`
deckt ausschließlich die Transkriptionsrichtung ab (Audio rein), nicht die
Synthese-Richtung (Text/Antwort raus als Audio) - das Dokument selbst
benennt das in Abschnitt 42 als separat zu untersuchende zweite Phase.
Vermutlich relevant, aber nicht verifiziert: der `stt-tts`/`managed-room`-
Pfad bzw. `talk.client.create` statt `talk.session.create`.

- Setzt 0.2.5 voraus (derselbe `audio_pipeline = "gateway"`-Schalter),
  ersetzt bei Aktivierung den lokalen Piper-Aufruf durch vom Gateway
  synthetisiertes Audio.
- `audio_pipeline = "local"` bleibt vollwertiger Pfad, keine Abkündigung
  von Piper.

## 0.3.x - Testing & Projektstruktur

Bewusst erst nach dem **kompletten** 0.2.x-Block (inklusive der
Gateway-Audio-Migration 0.2.5/0.2.6), nicht dazwischengeschoben: Die
Gateway-Audio-Migration wird voraussichtlich `audio.rs`, `transcribe.rs`,
`tts.rs` und ggf. `vad.rs` grundlegend anders schneiden als eine reine
Transport-Umstellung für Textnachrichten - eine Restrukturierung vor
diesem Punkt (oder nur bis 0.2.2) müsste mit hoher Wahrscheinlichkeit ein
zweites Mal gemacht werden, sobald die Audio-Migration landet. Deshalb
bewusst erst, wenn die endgültige Form von 0.2.x feststeht.

Kein neues Nutzerverhalten, sondern Testabdeckung/Struktur für das in
0.2.x neu Gebaute nachziehen - bestehende Konvention: Unit-Tests direkt in
der jeweiligen Datei (`#[cfg(test)] mod tests`), Feature-/Integrationstests
in `tests/`. Struktur zuerst, Tests danach: erst `src/` und `tests/` für
den seit 0.2.x gewachsenen Umfang neu ordnen, dann die neuen Module und
Pfade in dieser Struktur testen - nicht andersherum, sonst müssten die
neuen Tests bei der Umstrukturierung gleich wieder mitverschoben werden.
(Die je Schritt in 0.2.x bereits geschriebenen Tests decken die Logik ab;
hier geht es um die große strukturelle Aufräumung und System-Level-
Regressionstests über den kompletten neuen Pfad.)

| Version | Thema |
|---|---|
| 0.3.0 | `src/`-Modulstruktur nach dem kompletten 0.2.x-Block neu ordnen: `openclaw.rs` wird zu einem Modul mit CLI- und WebSocket-Transport (z. B. `src/openclaw/{mod,cli,websocket}.rs`); je nachdem, wie 0.2.5/0.2.6 tatsächlich geschnitten wurden, betrifft das ggf. auch `audio.rs`/`transcribe.rs`/`tts.rs` (lokaler vs. Gateway-Pfad). Exakter Zuschnitt entscheidet sich an der tatsächlichen Größe und Form der 0.2.x-Module, nicht vorab festlegen. |
| 0.3.1 | `tests/`-Struktur vor den neuen Regressionstests festlegen: gemeinsame Test-Helfer (Stub-/Mock-Erzeugung wie in `tests/pipeline_with_stubs.rs`) in ein wiederverwendbares Modul auslagern, statt sie in jedem künftigen Feature-Test erneut zu duplizieren |
| 0.3.2 | Unit-Tests für das Ergebnis der `src/`-Restrukturierung (0.3.0) nachziehen, wo sich durch das Verschieben Lücken ergeben - die fachliche Logik selbst ist bereits aus den 0.2.x-Schritten getestet |
| 0.3.3 | System-Level-Regressionstest in `tests/` für den kompletten Pfad über alle in 0.2.x aktivierbaren Kombinationen (`transport` x `audio_pipeline`) hinweg, gegen ein Mock-Gateway statt echtem OpenClaw (analog zu `tests/pipeline_with_stubs.rs`) |

## 0.4.x - Packaging als Hintergrunddienst

Hintergrunddienst und Menüleisten-UI sind zwei unterschiedliche
Prozess-/UI-Konzepte und werden deshalb nicht in einem Schritt vermischt:
Erst das Lifecycle-Modell festlegen (0.4.0), dann den Dienst danach bauen
(0.4.1), dann die UI-Hülle darauf aufsetzen (0.4.2).

### 0.4.0 - Lifecycle-/Prozessarchitektur festlegen

Reine Entscheidung, noch keine Implementierung - Ergebnis ist die
Grundlage für 0.4.1 und 0.4.2:

- **Prozessmodell:** ein einzelner Prozess (Dienst und Menüleisten-UI in
  einer Binary) oder zwei getrennte Prozesse (Hintergrunddienst +
  separates Status-/UI-Programm)?
- **Start:** launchd `LaunchAgent` (Login Item) vs. manueller Start -
  läuft der Dienst automatisch nach Login/Reboot an?
- **Beendigung:** wie wird der Dienst sauber gestoppt (SIGTERM über
  launchd, expliziter Befehl über die UI, o. Ä.) - siehe die bestehenden
  Shutdown-Invarianten in [AGENTS.md](AGENTS.md#invarianten).
- **Verhalten bei Logout/Reboot:** startet der Dienst danach automatisch
  neu, oder bleibt er bis zum nächsten manuellen Start aus?
- **IPC zwischen Statusfenster und Dienst** (nur relevant, falls zwei
  Prozesse): eigener lokaler Unix-Socket oder ein anderer Kanal? (Der in
  0.5.x geplante Unix-Socket für agenteninitiierte Nachrichten kommt
  zeitlich später und ist ein anderer Zweck - beide unabhängig
  betrachten, auch wenn sich derselbe Mechanismus anbieten könnte.)

### 0.4.1 - Hintergrunddienst

Nach dem in 0.4.0 festgelegten Modell:

- Läuft ohne offenes Fenster, kein Dock-Icon-/Vordergrund-Zwang.
- Start/Stop/Neustart bei Logout/Reboot entsprechend der 0.4.0-Festlegung.

### 0.4.2 - Menüleisten-Icon

Nach dem in 0.4.0 festgelegten Modell, auf 0.4.1 aufsetzend:

- Lebt dauerhaft als macOS-Menüleisten-Icon.
- Ein optionales Status-/Konfigurationsfenster kommuniziert mit dem Dienst
  über den in 0.4.0 festgelegten IPC-Weg - **Schließen des Fensters
  beendet den Dienst nicht**, beide sind entkoppelt.
- An/Aus-Toggle zum kompletten Aktivieren/Deaktivieren des Dienstes.

## 0.5.x - Agenteninitiierte Nachrichten (Auslöser bei OpenClaw agnostisch)

Bewusst erst nach vollständig integriertem 0.4.x und als **in sich
abgeschlossener Block** - nicht nebenbei in ein anderes Thema
eingestreut, mit eigener Struktur und eigenen Tests statt geteilt mit
0.3.x.

**Scope-Klarstellung:** Es geht nicht um "Cronjobs" im engeren Sinn,
sondern um jede Nachricht, die der Agent ohne eine vorherige Nutzer-
Nachricht verschickt - unabhängig davon, was sie auf OpenClaw-Seite
auslöst (Cron, Webhook, manueller Trigger, o. Ä.). Was dort triggert, ist
OpenClaws Sache; die Bridge bekommt davon nichts mit und muss dafür
agnostisch sein - jede eingehende Nachricht sieht für sie identisch aus,
unabhängig von ihrem Auslöser.

```
Beliebiger Auslöser (OpenClaw-Sache) -> OpenClaw -> voicewake emit
   -> Unix-Socket -> Rust-Queue -> Piper
```

### 0.5.0 - Schnittstelle festlegen

Fester lokaler Unix-Socket, Nachrichtenformat, Kommandos `notify`
(Direktansage) und `ask` (agentische Ansage) - unabhängig vom Auslöser
auf OpenClaw-Seite formuliert, nicht cron-spezifisch.

### 0.5.1 - Implementierung

Socket-Listener -> Rust-Queue -> Piper.

### 0.5.2 - Prioritäts-/Sprech-Queue

Warnungen laufen vor normalen Fertigmeldungen, nichts spricht parallel -
erst jetzt wirklich nötig, weil eine agenteninitiierte Nachricht (0.5.1)
mitten in eine laufende Konversation (0.2.1) platzen kann.

### 0.5.3 - Tests

In sich abgeschlossen statt Teil von 0.3.x: Unit-Tests direkt in den
neuen Dateien, ein Feature-Test in `tests/` für den kompletten Pfad
Socket -> Queue -> (gestubbtes) Piper.
