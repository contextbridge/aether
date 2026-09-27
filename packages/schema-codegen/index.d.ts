export interface GenerateTypesOptions {
  /** `cargo run -q` arguments. */
  cargo: string[];

  /** Declaration file to write. */
  output: string;

  /**
   * Module that declares the emitter's `external` types.
   */
  external?: { from: string; schema: string };

  /** Module that declares the types `tsType` schemas name, such as `wasm-bindgen` classes. */
  tsTypesFrom?: string;
}

/**
 * Runs a Rust schema emitter with `cargo run` and writes TypeScript declarations for the types it prints.
 */
export function generateTypes(options: GenerateTypesOptions): Promise<void>;
