import { compat, types as T } from "../deps.ts";

export const setConfig: T.ExpectedExports.setConfig = async (
  effects,
  input
) => {
  // deno-lint-ignore no-explicit-any
  const config = input as any;

  // Only require LND dependency if LND backend is selected
  const lightningBackend = config?.["gatewayd-lightning-backend"]?.["backend-type"];

  if (lightningBackend === "lnd") {
    const depsLnd: T.DependsOn = { lnd: ["synced"] };
    return await compat.setConfig(effects, input, depsLnd);
  }

  // For LDK, no additional dependencies required
  return await compat.setConfig(effects, input);
};
