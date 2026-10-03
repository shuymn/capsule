---
status: accepted
---

# Own acquisition in each zsh session

Use one worker per zsh session so generation, environment, cancellation, and reload share a single lifetime while input remains independent of acquisition. Accept one process per shell to keep service coordination and shared caches outside the runtime. Separate immutable ConfigPlan, bounded acquisition, and pure structured View as specified in [architecture.md](../architecture.md).
