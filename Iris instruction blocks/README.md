# These are historical build instructions, not a plan

**Everything described in this folder is done.** These are the original
per-block implementation instructions written in **March 2026**, kept as a
record of how Iris was specified and built. They are phrased as objectives —
*"Objective: Implement the iris-ui crate…"* — which reads like a to-do list for
an unbuilt application. It is not one.

For what Iris actually is and does **today**, read, in this order:

| file | what it tells you |
|---|---|
| `../README.md` | what Iris is, how to build, test, run and install it |
| `../ROADMAP.md` | the **only** authoritative list of what is not yet built |
| `../CHANGELOG.md` | what changed and when |

## Two things to know before reading further

**Some of these blocks describe crates that are still placeholders.** Blocks F-1
(`iris-control`) and G-1 (`iris-stream`) specify substantial components that
were never built. Those two gaps are declared in `../ROADMAP.md`; the crates
exist and deliberately expose no API. Do not read those blocks as descriptions
of working code.

**"BSR" is a sibling project**, referenced throughout as the design Iris was
modelled on and diverged from. It is not part of this repository and nothing
here depends on it. The references are historical context for design decisions,
not a dependency.

## Why this folder was kept

Deleting it would lose the reasoning behind a working system. The risk it
carries is the opposite one — that a visitor reads a March specification as
current status and concludes the app is unfinished. That is a documented trap in
this estate: a stale `missing_parts.md` shipped in a sibling repository scored a
finished app 2/5 and listed completed work as P0. This header exists so the same
mistake is not available here.

Added 2026-08-31.
