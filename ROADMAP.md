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
| 0.1.9 | Verwaiste Kindprozesse bei Timeout - prüfen, was über das bestehende `kill_on_drop` hinaus noch fehlt |
| 0.1.10 | Wake-Word- und TTS-Zustände gegen Echo und Doppeltrigger absichern |
| 0.1.11 | Adaptiver RMS-Schwellwert statt fixem `silence_rms_threshold` - Wurzel hinter dem Transkript-Filter (Dauergeräusch wie ein laufender Fernseher gilt sonst als Sprache) |
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

### 0.2.1 - Volle Integration

- `transport = "websocket"` nutzt `chat.send` statt `openclaw agent --json`
  - laut Doku non-blocking, mit sofortigem ACK (`runId`, `status: started`).
- Das ACK löst sofort eine Zwischenmeldung über Piper aus (Pendant zum
  "Ich schau mir das an" aus Telegram).
- `deltaText`-Events werden zu sprechbaren Abschnitten gesammelt, `final`
  schließt die Runde ab.
- `transport = "cli"` bleibt vollwertiger Legacy-Pfad, nicht nur ein
  Fallback - beide Transporte werden dauerhaft unterstützt.

### 0.2.2 - Lokaler Unix-Socket für OpenClaw-Cronjobs

```
OpenClaw-Cron -> voicewake emit -> Unix-Socket -> Rust-Queue -> Piper
```

Kommandos `notify` (Direktansage) und `ask` (agentische Ansage).

### 0.2.3 - Prioritäts-/Sprech-Queue

Warnungen laufen vor normalen Fertigmeldungen, nichts spricht parallel -
nötig, sobald Streaming-Antworten (0.2.1) und Cron-Events (0.2.2)
gleichzeitig eintreffen können.

## 0.3.x - Testing

Kein neues Nutzerverhalten, sondern Testabdeckung für das in 0.2.x neu
Gebaute nachziehen - bestehende Konvention: Unit-Tests direkt in der
jeweiligen Datei (`#[cfg(test)] mod tests`), Feature-/Integrationstests in
`tests/`.

| Version | Thema |
|---|---|
| 0.3.0 | Unit-Tests für die in 0.2.x neuen Module (WebSocket-Client, Unix-Socket-Listener, Prioritäts-Queue) direkt in den jeweiligen Dateien |
| 0.3.1 | Feature-Test in `tests/` für den WebSocket-Pfad, analog zu `tests/pipeline_with_stubs.rs` (Mock-Gateway statt echtem OpenClaw) |
| 0.3.2 | Feature-Test in `tests/` für den Unix-Socket-Eingang (simulierter Cron-`notify`/`ask`-Aufruf Ende-zu-Ende) |

## 0.4.x - Packaging als Hintergrunddienst

### 0.4.0 - Vollständiger Hintergrundtask

- Läuft ohne offenes Fenster, kein Dock-Icon-/Vordergrund-Zwang.
- Ein Status-/Konfigurationsfenster darf sich bei Bedarf öffnen lassen -
  **Schließen des Fensters beendet den Dienst nicht**, beide sind
  entkoppelt.

### 0.4.1 - Menüleisten-Icon

- Lebt dauerhaft als Menüleisten-Icon (macOS-Pendant zur rechten
  Windows-Taskleiste), vergleichbar mit der Tailscale-Menüleisten-App.
- An/Aus-Toggle zum kompletten Aktivieren/Deaktivieren des Dienstes.
