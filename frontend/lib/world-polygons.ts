// cspell:word topo
import { feature } from "topojson-client";
import topology from "world-atlas/countries-110m.json";
import countries from "world-countries";
import { type IsoCurrencyCode, SUPPORTED } from "./currencies";

const supportedSet = new Set<string>(SUPPORTED);

const idToCountryInfo = new Map<
  string,
  { cca2: string; cca3: string; currencies: IsoCurrencyCode[] }
>();
for (const c of countries) {
  const supported = Object.keys(c.currencies ?? {}).filter((k) =>
    supportedSet.has(k),
  ) as IsoCurrencyCode[];
  // world-atlas's countries-110m uses 3-digit zero-padded ISO numeric codes
  // ("076" for Brazil, "840" for USA). world-countries' `ccn3` is already in
  // the same padded form, so use it directly as the join key.
  idToCountryInfo.set(c.ccn3, {
    cca2: c.cca2,
    cca3: c.cca3,
    currencies: supported,
  });
}

// topojson-client typings narrow on the second arg; cast through `any` so the
// GeometryCollection → FeatureCollection overload is selected.
// biome-ignore lint/suspicious/noExplicitAny: see comment above
const topo = topology as any;
// biome-ignore lint/suspicious/noExplicitAny: see comment above
const fc = feature(topo, topo.objects.countries) as any;

export type CountryFeature = {
  type: "Feature";
  geometry: GeoJSON.Geometry;
  properties: {
    name: string;
    cca2: string;
    cca3: string;
    currencies: IsoCurrencyCode[];
  };
  id?: string | number;
};

export const WORLD_POLYGONS: CountryFeature[] = (
  fc.features as Array<{
    geometry: GeoJSON.Geometry;
    id?: string | number;
    properties?: { name?: string } | null;
  }>
).map((f) => {
  const id = f.id;
  const info = idToCountryInfo.get(String(id));
  return {
    type: "Feature",
    geometry: f.geometry,
    id,
    properties: {
      name: f.properties?.name ?? "",
      cca2: info?.cca2 ?? "",
      cca3: info?.cca3 ?? "",
      currencies: info?.currencies ?? [],
    },
  };
});
