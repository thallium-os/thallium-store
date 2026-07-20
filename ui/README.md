# Thallium Store UI

Run from the repository root after building the backend:

```bash
PATH="$PWD/target/debug:$PATH" THALLIUM_STORE_FAKE_UNI=1 quickshell --path ui/shell.qml
```

The UI starts `thallium-store-backend` from `PATH`.
