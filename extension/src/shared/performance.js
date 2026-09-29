export const PERFORMANCE_BUDGETS_MS = Object.freeze({
  popup_input_ready: 100,
  native_connection: 250,
  first_response_chunk: 1500
});

export const PERFORMANCE_MARKS = Object.freeze({
  popup_input_ready: "tabbeam.popup.input-ready",
  native_connection: "tabbeam.native.connection",
  first_response_chunk: "tabbeam.response.first-chunk"
});

/**
 * Records a local browser PerformanceEntry when supported. No prompt or
 * provider content is attached.
 *
 * @param {keyof typeof PERFORMANCE_MARKS} metric
 * @param {number} start
 * @param {number} end
 * @param {Performance | undefined} [performanceApi]
 */
export function recordDuration(metric, start, end, performanceApi = globalThis.performance) {
  const duration = Math.max(0, end - start);
  try {
    performanceApi?.measure(PERFORMANCE_MARKS[metric], { start, end });
  } catch {
    // Timing telemetry must never break the product.
  }
  return duration;
}

/** @param {keyof typeof PERFORMANCE_BUDGETS_MS} metric @param {number} duration */
export function withinPerformanceBudget(metric, duration) {
  return duration <= PERFORMANCE_BUDGETS_MS[metric];
}
