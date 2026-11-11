# rail-test-support

Helpers the workspace's tests share, so every crate tests its settings and its binary the same way. Nothing shipped uses it.

What the workspace's tests share, as a dev-dependency only: settings read as a binary reads them, and a binary run as a server, stopped and read back.
