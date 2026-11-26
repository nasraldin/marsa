import assert from "node:assert/strict";
import { GenericContainer } from "testcontainers";

const container = await new GenericContainer(process.env.MARSA_SAMPLE_IMAGE)
  .withCommand(["sleep", "180"])
  .start();
try {
  const result = await container.exec(["echo", "marsa-node"]);
  assert.equal(result.exitCode, 0);
  assert.equal(result.output.trim(), "marsa-node");
} finally {
  await container.stop();
}
