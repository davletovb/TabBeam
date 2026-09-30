# TabBeam

**TabBeam: On-demand, provider-independent AI for the tab or selection you explicitly share.**

TabBeam is a lightweight, provider-independent browser AI command layer.

The product is designed around a fast browser workflow:

> **Invoke → Ask → Read → Continue browsing**

and, when deeper work is needed:

> **Invoke → Ask → Deepen → Continue in full view**

TabBeam uses a thin Chrome Manifest V3 extension, its own Rust conversation
integration, and one shared Seatline companion for provider execution. Conclave
and other authorized apps can use the same Seatline installation.

See [shared companion setup](docs/architecture/shared-companion.md) for the
current development integration and the existing standalone packaging mode.

## Project status

The repository is currently in the **planning / foundation** stage. Implementation should follow the tracked milestone sequence rather than prematurely building generalized abstractions.

## Canonical documents

- [Product & Engineering Framework](docs/product/browser-ai-extension-framework.md)
- [Implementation Plan & Tracker](docs/product/browser-ai-implementation-plan-tracker.md)
- [ADR-0001: Write the native host in Rust](docs/architecture/adr-0001-native-host-in-rust.md)

The implementation tracker is the source of truth for work sequencing, dependencies, acceptance criteria, verification, and milestone status.

## Planned architecture

| Component | Owns |
| --- | --- |
| TabBeam extension | Browser UI, explicit page capture, conversation history |
| TabBeam native integration | TabBeam protocol and conversation/session policy |
| Seatline companion | Shared provider execution, scheduling, app authorization and scoped IPC |

## Implementation principle

Build the product before extracting a framework:

1. prove the browser ↔ native round trip;
2. integrate one real provider;
3. add conversation continuity and browser context;
4. prove the provider abstraction with a second provider;
5. only then extract reusable native primitives into a library crate.

See the tracker for the complete execution order.

## License

No license has been selected yet.
