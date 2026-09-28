import { DIAGNOSTICS_MESSAGE, buildDiagnosticsExport } from "../shared/diagnostics.js";
import { providerLabel } from "../shared/providers.js";

/**
 * @param {{
 *   details: HTMLDetailsElement,
 *   refresh: HTMLButtonElement,
 *   export: HTMLButtonElement,
 *   host: HTMLElement,
 *   protocol: HTMLElement,
 *   provider: HTMLElement,
 *   failure: HTMLElement
 * }} elements
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 */
export function bindDiagnostics(elements, runtime) {
  let pending = false;

  async function refresh() {
    if (pending) return;
    pending = true;
    elements.refresh.disabled = true;
    try {
      const summary = await runtime.sendMessage({ type: DIAGNOSTICS_MESSAGE });
      renderDiagnostics(elements, summary);
    } catch {
      elements.host.textContent = "Unavailable";
      elements.protocol.textContent = "Unavailable";
      elements.provider.textContent = "Unavailable";
      elements.failure.textContent = "Diagnostics could not be loaded.";
    } finally {
      pending = false;
      elements.refresh.disabled = false;
    }
  }

  elements.refresh.addEventListener("click", () => { void refresh(); });
  elements.export.addEventListener("click", async () => {
    if (pending) return;
    pending = true;
    elements.refresh.disabled = true;
    elements.export.disabled = true;
    try {
      const summary = await runtime.sendMessage({ type: DIAGNOSTICS_MESSAGE });
      renderDiagnostics(elements, summary);
      downloadDiagnosticsExport(buildDiagnosticsExport(summary));
    } catch {
      elements.failure.textContent = "Diagnostics export could not be created.";
    } finally {
      pending = false;
      elements.refresh.disabled = false;
      elements.export.disabled = false;
    }
  });
  elements.details.addEventListener("toggle", () => {
    if (elements.details.open) void refresh();
  });

  return { refresh };
}

/** @param {any} elements @param {any} summary */
export function renderDiagnostics(elements, summary) {
  const host = summary?.host;
  if (host?.state === "available") {
    elements.host.textContent = host.version ? "Companion " + host.version : "Companion available";
  } else if (host?.state === "not-installed") {
    elements.host.textContent = "Companion not installed or registered";
  } else if (host?.state === "unavailable") {
    elements.host.textContent = "Companion unavailable";
  } else {
    elements.host.textContent = "Companion not checked";
  }

  const extensionProtocol = Number.isInteger(summary?.protocol_version)
    ? String(summary.protocol_version)
    : "unknown";
  const hostProtocols = Array.isArray(host?.protocol_versions) && host.protocol_versions.length
    ? host.protocol_versions.join(", ")
    : "unknown";
  elements.protocol.textContent = "Extension v" + extensionProtocol + "; companion: " + hostProtocols;

  const provider = summary?.provider;
  const label = typeof provider?.provider_id === "string"
    ? providerLabel(provider.provider_id)
    : "Provider";
  elements.provider.textContent =
    label + ": " + (provider?.availability ?? "unknown") + ", " + (provider?.authentication ?? "unknown");

  const failure = summary?.recent_failure;
  elements.failure.textContent = failure
    ? failure.code + " / " + failure.reason
    : "None";
}


/** @param {any} bundle */
export function downloadDiagnosticsExport(bundle) {
  const blob = new globalThis.Blob([JSON.stringify(bundle, null, 2) + "\n"], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  try {
    const link = document.createElement("a");
    link.href = url;
    link.download = "pervue-support-diagnostics.json";
    link.rel = "noopener";
    link.click();
  } finally {
    URL.revokeObjectURL(url);
  }
}
