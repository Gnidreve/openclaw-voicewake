# Ideen & offene Punkte

Gesammelte Notizen aus der Diskussion zum aktuellen Stand von voicewake und
möglichen nächsten Schritten. Kein Anspruch auf Vollständigkeit - dient als
Ausgangspunkt für künftige Iterationen.

## Bereits außerhalb der Rust-Binary umgesetzt

- `hey_jarvis` als lokales Wakeword über openWakeWord eingebunden.
- Fehler im Listener behoben: ffmpeg liefert kleine Live-Pakete; der Listener
  sammelt sie jetzt zu vollständigen Audioframes.
- Anker PowerConf S330 als Aufnahmegerät konfiguriert.
- Whisper large-v3-turbo lokal angebunden.
- Piper-Adapter gebaut, der die Schnittstelle der Rust-Binary auf das
  vorhandene Piper-Script übersetzt.
- OpenClaw-Adapter gebaut, der die separate Session
  `agent:main:voice-assistant` nutzt und die Vorleseregel ohne Emojis
  ergänzt.
- Die kompilierte Rust-Binary aus dem GitHub-Artifact blieb dabei
  unverändert - alles oben ist Config/Adapter-seitig gelöst.

## Empfohlene Verbesserungen direkt am Rust-Code

1. Start-Glassound **vor** dem Öffnen des Mikrofons abspielen. Aktuell kann
   der Ton bei einem Lautsprecher mit integriertem Mikrofon (z. B. Anker
   PowerConf S330) in der eigenen Aufnahme landen.
2. Audioaufnahme robuster machen: Ringbuffer statt Allokationen im
   Echtzeit-Callback; stabile Behandlung von Sample-Rate und Kanalzahl.
3. Shutdown/Abbruch in **allen** Phasen unterstützen: Wakeword, Aufnahme,
   Whisper, OpenClaw und TTS (bisher primär für die Wakeword-Wartephase
   gelöst).
4. Kindprozesse sauber beenden und bei Timeouts auch verwaiste
   OpenClaw-/ffmpeg-Prozesse aufräumen.
5. Wakeword- und TTS-Zustände gegen Echo und Doppeltrigger absichern.
6. Konfiguration und Adapter genauer validieren, Fehlerausgaben nicht
   verschlucken.
7. Streaming-Unterstützung ergänzen. Die aktuelle CLI liefert nur eine
   fertige Antwort; echtes Streaming braucht Gateway-Events oder einen
   Streaming-Endpunkt sowie eine Rust-Queue für Antwort-Chunks.
8. Lokalen Unix-Socket einbauen, damit OpenClaw-Cronjobs Ereignisse senden
   können:

   ```
   OpenClaw-Cron -> voicewake emit -> Unix-Socket -> Rust-Queue -> Piper
   ```

   Dafür wären `notify` (Direktansagen) und `ask` (agentische Ansagen)
   sinnvolle Kommandos.
9. Prioritäts-/Sprech-Queue einbauen, damit Warnungen vor normalen
   Fertigmeldungen gesprochen werden und nichts parallel läuft.
10. Erst danach als dauerhaften macOS-Hintergrunddienst bzw. echtes
    OpenClaw-Plugin paketieren.
11. ~~Dateien aufräumen, nachdem sie ihren Zweck erfüllt haben - nicht erst
    pauschal am Ende des gesamten Zyklus.~~ **Umgesetzt:** Rohaufnahme wird
    direkt nach der ffmpeg-Normalisierung gelöscht, die normalisierte
    Aufnahme direkt nach der Transkription (jeweils vor dem OpenClaw-
    Aufruf, unabhängig von dessen Ausgang) - Whisper- und Piper-
    Zwischendateien waren schon vorher direkt nach Gebrauch gelöscht
    worden. Nach einem erfolgreichen Durchlauf liegt damit keine
    Audiodatei mehr im temporären Verzeichnis; bei einem fehlgeschlagenen
    Zyklus räumt weiterhin die bestehende Zyklusende-Bereinigung des
    gesamten temporären Verzeichnisses auf.
12. ~~Leere Audioaufnahmen (Stille) nicht an den Agent weiterreichen.~~
    **Umgesetzt (Variante "vorab prüfen"):** Reproduzierter Bug aus dem
    Feldtest - Whisper halluziniert bei reiner Stille/Hintergrundrauschen
    teils nicht-leeren Text, wodurch das bisherige
    "leeres-Transkript-danach"-Kriterium nicht griff und die
    Folgerunden-Schleife sich an sich selbst hochschaukelte. Die VAD
    (`SilenceTracker`) weiß bereits während der Aufnahme, ob jemals
    echte Sprache (`speech_started`) erkannt wurde; wurde nie Sprache
    erkannt, werden ffmpeg-Normalisierung und whisper-cli jetzt komplett
    übersprungen, statt Whisper aus Stille etwas heraushalluzinieren zu
    lassen. Hängt weiterhin an Punkt 14 (Hintergrundgeräusch über dem
    RMS-Schwellwert würde `speech_started` fälschlich auslösen).
13. Für Fehler braucht es einen weiteren, vom Bestätigungston (Glass)
    unterscheidbaren Sound - damit man akustisch sofort erkennt, ob ein
    Zyklus normal beendet wurde oder ein Fehler aufgetreten ist, statt nur
    stumm zurück nach `IDLE` zu gehen bzw. es nur im Log zu sehen.
14. Bei ganz leichten Hintergrundgeräuschen wird das Schließen des
    Audio-Channels unterbunden: Liegt die Umgebungslautstärke dauerhaft
    knapp über `silence_rms_threshold`, erkennt die VAD nie "Stille" und
    der Silence-Timeout greift nicht - die Aufnahme läuft bis zum viel
    späteren `max_recording_seconds`-Limit weiter, statt nach der
    erwarteten kurzen Stille zu enden. Muss noch überlegt werden, wie sich
    das minimieren lässt (z. B. adaptiver/dynamischer Schwellwert statt
    fixem `silence_rms_threshold`, oder Rauschprofil zu Beginn der
    Aufnahme kalibrieren).
15. Wie und wann eine neue OpenClaw-Session gestartet wird, damit eine
    Session nicht unendlich weiterläuft (Kontext wächst sonst
    unbegrenzt). Grobe Idee, noch nicht final: Ist die letzte Nachricht
    länger als eine Stunde her, wird vor dem nächsten Aufruf erst ein
    Session-Reset (z. B. `/new` o. Ä. - genaue Umsetzung offen) gesendet.
    Schwelle und Mechanismus müssen noch entschieden werden.

**Kurz gesagt:** Die Basis funktioniert. Der nächste echte Rust-Schritt ist
nicht noch mehr KI, sondern Prozesssteuerung, Event-Eingang, Queueing und
Streaming.

## Zwischenmeldungen ("Okay, ich schau mir das an ...")

Gewünschtes Verhalten (wie früher in Telegram): nach Eingabe sofort eine
Empfangsbestätigung, optional Zwischenstatus während der Bearbeitung, dann
erst die eigentliche Antwort - statt nur eines einzelnen finalen Ergebnisses.

- Das ist **kein** Token-Streaming, sondern mehrere getrennte Nachrichten
  (Dispatches).
- Der aktuelle Aufruf `openclaw agent --json` ist ein synchroner RPC: eine
  Eingabe rein, eine fertige JSON-Antwort zurück. `--deliver` liefert die
  fertige Antwort aus, schaltet aber keine Zwischenmeldungen frei.
- **Ohne Rust-Neubuild näherungsweise machbar:** Der OpenClaw-Adapter spricht
  vor dem eigentlichen CLI-Aufruf sofort eine feste Empfangsbestätigung über
  Piper. Danach läuft der Agent-Aufruf wie bisher, die finale Antwort geht
  wie gewohnt an Rust/Piper.
- Für **echte**, agentengenerierte Zwischenmeldungen (nicht nur eine feste
  Phrase) braucht es Gateway-Events statt des CLI-RPC-Aufrufs.

## Untersuchung: OpenClaw-Gateway-WebSocket für echtes Streaming-Verhalten

Frage: Lässt sich das aus Telegram bekannte Verhalten (Eingang der
Voice-Message, laufende Agent-Session, separate Zwischen-/Antwort-Dispatches)
auch für voicewake erreichen?

Ergebnis der Prüfung von lokaler Gateway-Doku und installierter
Implementierung: **Ja, der Weg existiert.**

Relevanter WebSocket-Pfad:

1. Verbindung zu `ws://127.0.0.1:18789` (Gateway läuft lokal auf Loopback).
2. `connect`-Handshake als lokaler Operator mit passenden Scopes.
3. `sessions.messages.subscribe` für `agent:main:voice-assistant`.
4. `chat.send` statt `openclaw agent --json`.
5. Sofortiges ACK mit `runId` und `status: started`.
6. Danach laufende Gateway-Events:
   - `chat` mit `deltaText` und später `final`
   - `session.message`
   - `session.operation`
   - `session.tool` (laufender Tool-/Arbeitsfortschritt)

Laut Doku ist `chat.send` non-blocking; die Antwort streamt über
`chat`-Events - das entspricht dem aus Telegram bekannten Verhalten.

**Wichtig:** Bei dieser Untersuchung wurden nur die vorhandenen
Schnittstellen gesichtet, nichts am Gateway geändert oder neu gestartet.

### Möglicher erster Schritt (ohne Rust-Neubuild)

Der OpenClaw-Adapter könnte per WebSocket `chat.send` starten, beim ACK
sofort "Ich schaue mir das an" an Piper geben, und danach Delta-/
Fortschritts-Events zu sprechbaren Abschnitten sammeln. Die finale Antwort
würde weiterhin wie bisher an Rust übergeben.

Für sauberes Queueing, Cron-Events und parallele Ansagen bleibt eine
spätere Rust-Integration (siehe Punkte 7-9 oben) trotzdem sinnvoller als ein
reiner Adapter-Workaround.

### Nächster sinnvoller Schritt

Ein kleiner **read-only Streaming-Prototyp** im Adapter, der die
WebSocket-Events zunächst nur protokolliert (`chat`, `session.message`,
`session.operation`, `session.tool`) - bevor Audio/Piper daran gehängt wird.

## Paketierung: Hintergrunddienst mit Menüleisten-Icon (wie Tailscale)

Ergänzung zu Punkt 10 oben ("als dauerhaften macOS-Hintergrunddienst
paketieren") - konkrete Anforderungen an das spätere Packaging:

- Muss ein **vollständiger Hintergrundtask** sein: läuft ohne offenes
  Fenster, kein Dock-Icon/Vordergrund-App-Zwang.
- Ein Fenster (Status/Konfiguration) soll sich bei Bedarf öffnen lassen -
  **Schließen des Fensters darf den Task aber nicht beenden**. Fenster
  und Hintergrunddienst sind entkoppelt.
- Lebt dauerhaft als **Menüleisten-Icon** (macOS-Pendant zur rechten
  Windows-Taskleiste), vergleichbar mit der Tailscale-Menüleisten-App.
- Über das Menüleisten-Icon: **An/Aus-Toggle** zum kompletten
  Aktivieren/Deaktivieren des Dienstes.
