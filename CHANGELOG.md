# Changelog

All notable changes to Climax. Format based on [Keep a Changelog](https://keepachangelog.com/).

## [1.1.0] - 2026-10-15

### Added

- **A ready-made Docker image for Climax Server.** `ghcr.io/pineapplestorm/climax` is published with every release, for amd64 and arm64, so Climax Server no longer has to be built from source. `docker-compose.yml` uses it, and its `climax` service can be pasted straight into a compose file you already have.
- **Keep empty sessions** (Settings - Smart tracking). A session with no scenes and nothing logged is normally discarded when you stop it. Turn this on to keep it, so Climax can be used as a plain timer without Stash. Off by default.
- **Log an O with no scene straight from the dashboard.** The Overview session card has the same button the tracker window has, beside Start, Pause and Stop. It works with or without a session running and stamps the real time. This is what makes it possible from a browser, where there is no tracker window.
- **Wording** (Settings - General). Choose whether the app says "cumshot" or "orgasm". It changes what you see everywhere, including the tray tooltip and CSV exports, and nothing else.

### Changed

- The wrap-up now opens whenever a session has anything logged, not only when it has a scene. A session with only no-scene O's used to save the moment you pressed Stop, with no chance to add a note or adjust the count.
- With Keep empty sessions on, the wrap-up also opens for a session with nothing in it, so there is a place to log one, add a note or discard it.

### Fixed

- **A restart no longer wipes a session that had no scene playing.** Climax decided whether to carry a session across a restart by how long ago a scene last played. A session with no scene never updated that, so ten minutes in, any restart (a Climax Server update, a NAS reboot, a crash) treated it as abandoned: Climax Server ended it at its start time with nothing counted, and the desktop app offered to discard the whole of it. A running session is now marked alive every 30 seconds whether or not anything is playing.
- **An O logged in the first moments of a session no longer disappears from its timeline.** It was drawn directly underneath the session's start-time label, which sits on top, so the session said 3 and the timeline showed 2. It now sits just below the label. Time labels elsewhere on the timeline also give way a little sooner, so an O near one is never partly tucked under it.

## [1.0.0] - 2026-08-15

First release.
