# Known Limitations

- Retrying a failed operation is not implemented. `operations.retry` returns an
  error; start the install again from the app's page instead.
- Launching an installed app from the store is not implemented. `apps.launch`
  returns an error; use your normal launcher.
- Package changes are delegated to the bundled UNI wrapper, which drives the
  system's own `apt`, `flatpak` and `snap` commands. Progress is only as detailed
  as those tools report — some steps surface as a coarse percentage.
- APT results come from an in-memory index built at daemon startup, seeded from
  `apt-cache`. It is not AppStream metadata, so descriptions and artwork are
  thinner than a native AppStream store would show.
- Flatpak results come from the Flathub HTTP index; other configured remotes are
  only consulted for locally installed apps.
- GitHub metadata is fetched anonymously unless `GITHUB_TOKEN` is set. Anonymous
  access is capped at 60 requests/hour/IP, so heavy browsing can leave GitHub
  panels showing cached data until the quota resets.
- The bundled curated catalog is deliberately small; it exists so the store is
  useful on first launch, before any index has warmed up.
- The desktop-independent frontend opens in the system's default browser rather
  than a dedicated native window. It polls operation progress while its tab is
  open; closing the tab lets the local server exit after its idle timeout.
- Browser clients poll operation state rather than subscribing to the backend's
  Unix-socket progress stream. This adds a small delay (up to roughly two
  seconds) before a progress change appears in the UI.
