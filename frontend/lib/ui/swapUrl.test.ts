import { describe, expect, it } from "vitest";
import { readVaultPin, VAULT_LEADER_PARAM, VAULT_PIN_PARAM } from "./swapUrl";

describe("readVaultPin", () => {
  it("reads every pin key from a location.search string", () => {
    expect(
      readVaultPin(
        `?base=TGBP&quote=USDC&${VAULT_PIN_PARAM}=5&${VAULT_LEADER_PARAM}=So1`,
      ),
    ).toEqual({ base: "TGBP", quote: "USDC", vault: "5", leader: "So1" });
  });

  it("takes the string with or without its leading ?", () => {
    expect(readVaultPin("base=TGBP")).toEqual(readVaultPin("?base=TGBP"));
  });

  it("reads an empty search as no pin", () => {
    expect(readVaultPin("")).toEqual({
      base: "",
      quote: "",
      vault: "",
      leader: "",
    });
  });

  it("drops a vault number that isn't a decimal integer", () => {
    expect(readVaultPin(`?${VAULT_PIN_PARAM}=5x`).vault).toBe("");
  });

  it("ignores params outside the pin", () => {
    expect(readVaultPin("?seq=3&base=EURC")).toEqual({
      base: "EURC",
      quote: "",
      vault: "",
      leader: "",
    });
  });
});
