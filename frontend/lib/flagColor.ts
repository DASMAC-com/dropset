"use client";

import { useEffect, useState } from "react";

// Module-level cache of computed dominant colors keyed by the lookup id the
// caller provides (typically a currency code or cca2). Persists across
// renders / search filters so a row that re-renders doesn't repeat the
// canvas raster.
export type Rgb = [number, number, number];
const cache = new Map<string, Rgb | null>();

// Pixel-filter thresholds for picking a representative brand color from a
// flag SVG. Drops near-grey (low saturation), very dark, and very bright
// pixels so the average lands on a saturated brand band rather than skewing
// toward white/black bars.
const MIN_ALPHA = 200;
const MIN_SATURATION = 0.3;
const MIN_BRIGHTNESS = 60;
const MAX_BRIGHTNESS = 245;
const RASTER_SIZE = 24;

const sampleDominantColor = (
  ctx: CanvasRenderingContext2D,
  size: number,
): Rgb | null => {
  let r = 0;
  let g = 0;
  let b = 0;
  let n = 0;
  const { data } = ctx.getImageData(0, 0, size, size);
  for (let i = 0; i < data.length; i += 4) {
    const pa = data[i + 3];
    if (pa < MIN_ALPHA) continue;
    const pr = data[i];
    const pg = data[i + 1];
    const pb = data[i + 2];
    const max = Math.max(pr, pg, pb);
    const min = Math.min(pr, pg, pb);
    const sat = max === 0 ? 0 : (max - min) / max;
    if (sat < MIN_SATURATION) continue;
    if (max < MIN_BRIGHTNESS || max > MAX_BRIGHTNESS) continue;
    r += pr;
    g += pg;
    b += pb;
    n++;
  }
  if (n === 0) return null;
  return [(r / n) | 0, (g / n) | 0, (b / n) | 0];
};

export const computeFlagColor = (url: string): Promise<Rgb | null> => {
  if (typeof document === "undefined") return Promise.resolve(null);
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      const canvas = document.createElement("canvas");
      canvas.width = RASTER_SIZE;
      canvas.height = RASTER_SIZE;
      const ctx = canvas.getContext("2d", { willReadFrequently: true });
      if (!ctx) return resolve(null);
      ctx.clearRect(0, 0, RASTER_SIZE, RASTER_SIZE);
      ctx.drawImage(img, 0, 0, RASTER_SIZE, RASTER_SIZE);
      resolve(sampleDominantColor(ctx, RASTER_SIZE));
    };
    img.onerror = () => resolve(null);
    img.src = url;
  });
};

// React hook: returns the cached dominant color for `id`, computing it on
// first call and caching the result. The id is what the cache is keyed by
// — typically a currency code so different URLs that share an id (e.g.
// regional vs national flag for the same currency) collapse to one entry.
export const useFlagColor = (id: string, url: string): Rgb | null => {
  const [color, setColor] = useState<Rgb | null>(() =>
    cache.has(id) ? (cache.get(id) ?? null) : null,
  );
  useEffect(() => {
    if (cache.has(id)) {
      setColor(cache.get(id) ?? null);
      return;
    }
    let cancelled = false;
    computeFlagColor(url).then((c) => {
      if (cancelled) return;
      cache.set(id, c);
      setColor(c);
    });
    return () => {
      cancelled = true;
    };
  }, [id, url]);
  return color;
};
