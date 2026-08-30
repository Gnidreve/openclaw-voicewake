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

Läuft weiter, bis der Umstieg auf 0.2.x sauber möglich ist.

| Version | Thema |
|---|---|
| 0.1.12 | OpenClaw-Session-Reset nach Inaktivität (Schwelle und Mechanismus noch offen) |
| 0.1.13 | Ton für `[Output] skipped` (Kandidat: der bestehende Fehlerton) - abgeschickt, aber keine Antwort erhalten, bleibt sonst akustisch unbemerkt |
| 0.1.14 | Verbleibende Lücken in Config-/Adapter-Validierung schließen, Fehlerausgaben nicht verschlucken |

## 0.2.x - WebSocket-Streaming, CLI bleibt als Legacy-Pfad

Transport-Wahl **ausschließlich über Config**, kein CLI-Flag dafür:

```toml
[openclaw]
transport = "cli"  # oder "websocket" - Default "cli", bestehende Configs laufen unverändert weiter
```

### 0.2.0 - Read-only Streaming-Prototyp

Recherchierter Weg (Gateway-Doku + installierte Implementierung geprüft,
nichts am Gateway verändert oder neu gestartet):

1. Verbindung zu `ws://127.0.0.1:18789` (Gateway läuft lokal auf Loopback).
2. `connect`-Handshake als lokaler Operator mit passenden Scopes.
3. `sessions.messages.subscribe` für den konfigurierten Zielkanal.
4. Events zunächst nur protokollieren, noch kein Audio/Piper daran:
   `chat` (mit `deltaText` und später `final`), `session.message`,
   `session.operation`, `session.tool`.

**Bewusst rein lesend:** `chat.send` (aktives Auslösen einer Antwort über
den WebSocket) kommt erst in 0.2.1. Hier wird nur beobachtet, was das
Gateway an Events liefert - ausgelöst z. B. durch eine parallel laufende
CLI-Nutzung derselben Session, nicht durch diesen Prototyp selbst.

### 0.2.1 - Volle Integration

- `transport = "websocket"` nutzt `chat.send` statt `openclaw agent --json`
  - laut Doku non-blocking, mit sofortigem ACK (`runId`, `status: started`).
- Das ACK löst sofort eine Zwischenmeldung über Piper aus (Pendant zum
  "Ich schau mir das an" aus Telegram).
- `deltaText`-Events werden zu sprechbaren Abschnitten gesammelt, `final`
  schließt die Runde ab.
- `transport = "cli"` bleibt vollwertiger Legacy-Pfad, nicht nur ein
  Fallback - beide Transporte werden dauerhaft unterstützt.

## 0.3.x - Testing & Projektstruktur

Kein neues Nutzerverhalten, sondern Testabdeckung für das in 0.2.x neu
Gebaute nachziehen - bestehende Konvention: Unit-Tests direkt in der
jeweiligen Datei (`#[cfg(test)] mod tests`), Feature-/Integrationstests in
`tests/`. Struktur zuerst, Tests danach: erst `src/` und `tests/` für den
seit 0.2.x gewachsenen Umfang neu ordnen, dann die neuen Module und Pfade
in dieser Struktur testen - nicht andersherum, sonst müssten die neuen
Tests bei der Umstrukturierung gleich wieder mitverschoben werden.

| Version | Thema |
|---|---|
| 0.3.0 | `src/`-Modulstruktur nach 0.2.x neu ordnen: `openclaw.rs` wird zu einem Modul mit CLI- und WebSocket-Transport (z. B. `src/openclaw/{mod,cli,websocket}.rs`) statt einer wachsenden Einzeldatei. Exakter Zuschnitt entscheidet sich an der tatsächlichen Größe der 0.2.x-Module. |
| 0.3.1 | `tests/`-Struktur vor den neuen Feature-Tests festlegen: gemeinsame Test-Helfer (Stub-Erzeugung wie in `tests/pipeline_with_stubs.rs`) in ein wiederverwendbares Modul auslagern, statt sie in jedem künftigen Feature-Test erneut zu duplizieren |
| 0.3.2 | Unit-Tests für den in 0.2.x neuen WebSocket-Client direkt in den Dateien der neuen `src/`-Struktur |
| 0.3.3 | Feature-Test in `tests/` für den kompletten WebSocket-Pfad - nicht nur Verbindungsaufbau/Subscribe, sondern bis zur tatsächlichen Sprachausgabe: `chat.send` -> `deltaText`/`final`-Events -> Piper, gegen ein Mock-Gateway statt echtem OpenClaw (analog zu `tests/pipeline_with_stubs.rs`) |

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
