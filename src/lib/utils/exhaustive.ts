/** Compile-time exhaustiveness guard.
 *
 * Put it where a branch chain runs out of named cases. TypeScript only allows
 * the call when every variant of the union has been handled above, so adding a
 * variant in Rust (which regenerates the union) fails `npm run check` here
 * instead of falling into a default that quietly picks the wrong answer.
 *
 * It throws rather than returning a placeholder: the case is impossible by
 * typing, and a thrown error is easier to read than an inconsistent state
 * propagating through the UI. */
export function assertNever(value: never, context = "value"): never {
  throw new Error(`Unhandled ${context}: ${JSON.stringify(value)}`);
}
