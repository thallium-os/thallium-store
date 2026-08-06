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
- The QML client talks to the backend through `--request` invocations and polls
  for queue updates. Persistent socket streaming should replace this.
