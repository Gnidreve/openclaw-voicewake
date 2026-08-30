# Ideen & offene Punkte

Unsortierter Rohideen-Eimer: alles rein, was einem einfällt, ohne Anspruch
auf Vollständigkeit, Reihenfolge oder Umsetzung. Sobald eine Idee hier
konkret geplant wird (Version, Umfang), wandert sie in [`ROADMAP.md`](ROADMAP.md)
und wird hier gelöscht - Details zu diesem Ablauf stehen in
[`AGENTS.md`](AGENTS.md#ideen-roadmap-changelog). Bereits umgesetzte und
veröffentlichte Punkte stehen nicht mehr hier, sondern in
[`CHANGELOG.md`](CHANGELOG.md).

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

## Weitere Ideen (unkonkret)

- Wake-Word-Schwellwert: Im Feldtest lagen echte Treffer bei Scores von
  0.50-0.98, das Grundrauschen aber schon bei ~0.20 - mit `--threshold 0.1`
  löste der Listener sofort ohne gesprochenes Wake-Word aus. 0.5 ist der
  aktuelle Arbeitswert, in README und `config.example.toml` als
  Erfahrungswert dokumentiert. Der Schwellwert lebt im Listener, nicht in
  der Rust-Binary. Offen: ob der Listener seine Scores dauerhaft mitloggen
  sollte, um den Wert pro Raum/Mikrofon sauber einstellen zu können.
- Whisper-eigenes VAD (`--vad` mit Silero) würde Halluzinationen aus
  Nicht-Sprache an der Quelle abstellen, nicht nur nachträglich über den
  Transkript-Filter; über `whisper.extra_args` ohne Rust-Änderung testbar.
- Wake-Word-Listener nativ in Rust statt als externer Prozess: Der
  Neustart pro Zyklus war die Quelle mehrerer Feldtest-Fehler. In Rust wäre
  es ein einziger Mikrofon-Besitzer, kostet aber eine ONNX-Runtime und die
  Nachbildung der openWakeWord-Vorverarbeitung.

## Android Applikation für remote Verbindungen vom Handy
- Eigene Release Datei für Android getrennt vom eigentlichen macos release. Durch die implementierung der websocket verbindung sollte jede version und jedes zielsystem im modus websocket 
