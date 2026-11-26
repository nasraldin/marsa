# JetBrains Docker integration

Use a disposable IDE profile with the official JetBrains Docker plugin. The
operator supplies an IDE installation and plugin covered by their agreement.
The check uses a running isolated Marsa guest, a private `DOCKER_CONFIG`, and
an already-loaded arm64 image containing `sleep`. Launch an isolated Android
Studio instance with a compatible, previously acquired plugin, then run the
observer in a second terminal. The launcher uses a new empty project, home,
IDE caches, settings and plugin directory for this run.

```sh
python3 tests/conformance/jetbrains/launch.py "$MARSA_TEST_SOCKET" /path/to/clouds-docker-impl
python3 tests/conformance/jetbrains/observe.py "$MARSA_TEST_SOCKET" "$MARSA_TEST_IMAGE"
```

In the isolated IDE project, add a Docker connection under **Settings → Build,
Execution, Deployment → Docker**. Select **TCP socket** and enter the complete
`unix://` URL printed by the script in **Engine API URL**. In **Services**,
connect to that engine and open **Containers**. Stop the uniquely named
container printed by the script using the IDE's Stop action. The script
checks the resulting state on that same engine and removes its container.
The output can be attached to the conformance run with the IDE and plugin
versions. [JetBrains Docker connection settings](https://www.jetbrains.com/help/idea/settings-docker.html)
