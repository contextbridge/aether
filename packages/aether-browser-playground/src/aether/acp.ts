/**
 * Narrow an ACP union by its `type` tag. Each ACP union also has a catch-all `{ type: string }`
 * member for forward compatibility, which stops a plain `value.type === "text"` from narrowing.
 */
export function hasType<T extends { type: string }, K extends string>(
  value: T,
  type: K,
): value is Extract<T, { type: K }> {
  return value.type === type;
}
