// cspell:word latlng
import countries from "world-countries";
import { type IsoCurrencyCode, SUPPORTED } from "./currencies";

export type CountryPin = {
  cca2: string;
  name: string;
  lat: number;
  lng: number;
  currency: IsoCurrencyCode;
  area: number;
};

const supportedSet = new Set<string>(SUPPORTED);

export const COUNTRY_PINS: CountryPin[] = countries.flatMap((c) => {
  const code = Object.keys(c.currencies ?? {}).find((k) => supportedSet.has(k));
  if (!code) return [];
  return [
    {
      cca2: c.cca2,
      name: c.name.common,
      lat: c.latlng[0],
      lng: c.latlng[1],
      currency: code as IsoCurrencyCode,
      area: c.area ?? 0,
    },
  ];
});

const anchorByCurrency = new Map<IsoCurrencyCode, CountryPin>();
for (const pin of COUNTRY_PINS) {
  const prev = anchorByCurrency.get(pin.currency);
  if (!prev || pin.area > prev.area) {
    anchorByCurrency.set(pin.currency, pin);
  }
}

export const defaultAnchorCca2 = (code: IsoCurrencyCode): string => {
  const pin = anchorByCurrency.get(code);
  return pin ? pin.cca2 : "";
};

export const findPin = (cca2: string): CountryPin | undefined =>
  COUNTRY_PINS.find((p) => p.cca2 === cca2);
