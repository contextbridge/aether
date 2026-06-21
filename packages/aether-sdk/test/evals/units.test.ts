import { describe, expect, it } from "vitest";

import { Image } from "../../src/evals/containers/image.js";

describe("Image.parse", () => {
  it("splits name and tag", () => {
    expect(Image.parse("aether-sandbox:dev")).toEqual(
      new Image("aether-sandbox", "dev"),
    );
  });

  it("keeps a registry path and reads the trailing tag", () => {
    expect(Image.parse("ghcr.io/org/aether:sha")).toEqual(
      new Image("ghcr.io/org/aether", "sha"),
    );
  });

  it("defaults the tag to latest", () => {
    expect(Image.parse("aether-sandbox")).toEqual(new Image("aether-sandbox"));
  });

  it("formats image references", () => {
    expect(new Image("aether-sandbox", "dev").toString()).toBe(
      "aether-sandbox:dev",
    );
  });

  it("attaches Dockerfile build settings", () => {
    expect(
      Image.fromDockerfile("aether-sandbox:dev", {
        context: "/repo",
        dockerfile: "Dockerfile.eval",
        buildArgs: { AETHER_VERSION: "test" },
      }),
    ).toEqual(
      new Image("aether-sandbox", "dev", {
        context: "/repo",
        dockerfile: "Dockerfile.eval",
        buildArgs: { AETHER_VERSION: "test" },
      }),
    );
  });

  it("rejects malformed references", () => {
    expect(() => Image.parse(":latest")).toThrow(/invalid container image/);
    expect(() => Image.parse("aether:")).toThrow(/invalid container image/);
  });
});
