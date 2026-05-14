"use client";

import {
  Compass,
  Crosshair,
  Flag,
  Minus,
  Pause,
  Play,
  Plus,
} from "lucide-react";
import dynamic from "next/dynamic";
import {
  Component,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  BufferAttribute,
  BufferGeometry,
  MeshBasicMaterial,
  Points,
  PointsMaterial,
  type Scene,
} from "three";
import { COUNTRY_PINS, type CountryPin, findPin } from "@/lib/countries";
import {
  CURRENCIES,
  type IsoCurrencyCode,
  tokenIconUrl,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { useSwapStore } from "@/lib/store";
import { type CountryFeature, WORLD_POLYGONS } from "@/lib/world-polygons";
import { CurrencyGroupHeader } from "./CurrencyGroupHeader";

const Globe = dynamic(() => import("react-globe.gl"), {
  ssr: false,
  loading: () => (
    <div className="flex h-[480px] w-full items-center justify-center text-muted-fg text-sm">
      Loading globe…
    </div>
  ),
});

// Land tints — Sell = platform blue accent, Buy = emerald green.
const SELL_TINT = "#3b82f6"; // blue-500 (matches --accent)
const BUY_TINT = "#10b981"; // emerald-500
const LAND_COVERED = "#64748b"; // slate-500 — supports a stablecoin currency
const LAND_UNCOVERED = "#1e293b"; // slate-800 — no supported currency
const OCEAN_COLOR = 0x0b1726;
const ARC_COLOR = "#a7f3d0"; // emerald-200 — bright, ties the cool palette together

const POPOVER_WIDTH = 256;
const POPOVER_MAX_HEIGHT = 260;

// Start the view centered roughly over the eastern US so the auto-rotation
// reveals the Atlantic and then Europe — the canonical USD → EUR path.
const DEFAULT_POV = { lat: 30, lng: -75, altitude: 1.9 };

// Below this altitude, country-name labels become visible.
const LABEL_VISIBILITY_ALTITUDE = 2.6;

// Build a flag emoji from a 2-letter country code by mapping each letter to
// its Regional Indicator Symbol (🇺 + 🇸 → 🇺🇸). Pure data transform — works
// for any valid ISO 3166-1 alpha-2 code that the OS has emoji glyphs for.
const cca2ToFlag = (cca2: string): string => {
  const A = "A".charCodeAt(0);
  const RI = 0x1f1e6;
  return cca2
    .toUpperCase()
    .split("")
    .map((c) => String.fromCodePoint(RI + c.charCodeAt(0) - A))
    .join("");
};

type Pov = { lat: number; lng: number; altitude: number };
type GlobeHandle = {
  controls: () => { autoRotate: boolean; autoRotateSpeed: number };
  pointOfView: (pov?: Pov, durationMs?: number) => Pov;
  scene: () => Scene;
};

// Procedurally generated star layer — a Points object placed at a fixed
// world position, so as the OrbitControls camera moves the stars appear to
// drift across the sky, anchored to the scene.
function makeStarLayer({
  count,
  radius,
  size,
  opacity,
}: {
  count: number;
  radius: number;
  size: number;
  opacity: number;
}) {
  const positions = new Float32Array(count * 3);
  for (let i = 0; i < count; i++) {
    // Uniform sample on a sphere of given radius.
    const theta = Math.random() * 2 * Math.PI;
    const phi = Math.acos(2 * Math.random() - 1);
    positions[i * 3] = radius * Math.sin(phi) * Math.cos(theta);
    positions[i * 3 + 1] = radius * Math.sin(phi) * Math.sin(theta);
    positions[i * 3 + 2] = radius * Math.cos(phi);
  }
  const geom = new BufferGeometry();
  geom.setAttribute("position", new BufferAttribute(positions, 3));
  const mat = new PointsMaterial({
    color: 0xffffff,
    size,
    sizeAttenuation: true,
    transparent: true,
    opacity,
    depthWrite: false,
  });
  return new Points(geom, mat);
}

type ClickContext = {
  countryName: string;
  cca2: string;
  currencies: IsoCurrencyCode[];
  x: number;
  y: number;
};

// Pre-clustered subset of COUNTRY_PINS for use at the most zoomed-out
// label-visible altitude band: greedy area-weighted clustering drops the
// smaller pins inside any 4°-radius proximity cluster so dense regions
// (the Lesser Antilles, Eurozone microstates) don't pile labels on top of
// each other at the default view. Smaller territories return as soon as
// the camera zooms past the medium-close bucket.
const FAR_ZOOM_PROXIMITY_DEG = 4;
function angularDistanceDeg(a: CountryPin, b: CountryPin): number {
  const toRad = (d: number) => (d * Math.PI) / 180;
  const p1 = toRad(a.lat);
  const p2 = toRad(b.lat);
  const dp = toRad(b.lat - a.lat);
  const dl = toRad(b.lng - a.lng);
  const h =
    Math.sin(dp / 2) ** 2 + Math.cos(p1) * Math.cos(p2) * Math.sin(dl / 2) ** 2;
  return (2 * Math.atan2(Math.sqrt(h), Math.sqrt(1 - h)) * 180) / Math.PI;
}
const FAR_ZOOM_PINS: CountryPin[] = (() => {
  const byAreaDesc = [...COUNTRY_PINS].sort((a, b) => b.area - a.area);
  const primary: CountryPin[] = [];
  const covered = new Set<string>();
  for (const pin of byAreaDesc) {
    if (covered.has(pin.cca2)) continue;
    primary.push(pin);
    for (const other of COUNTRY_PINS) {
      if (other.cca2 === pin.cca2 || covered.has(other.cca2)) continue;
      if (angularDistanceDeg(pin, other) < FAR_ZOOM_PROXIMITY_DEG) {
        covered.add(other.cca2);
      }
    }
  }
  return primary;
})();

class GlobeErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: { componentStack?: string | null }) {
    console.error("[GlobePanel] crash:", error, info?.componentStack);
  }
  render() {
    if (this.state.error) {
      return (
        <div className="flex h-[480px] w-full flex-col items-center justify-center gap-2 p-4 text-center text-muted-fg text-sm">
          <span className="font-medium">Globe failed to load.</span>
          <code className="max-w-full overflow-auto rounded bg-background px-2 py-1 font-mono text-xs">
            {String(this.state.error?.message ?? this.state.error)}
          </code>
        </div>
      );
    }
    return this.props.children;
  }
}

function GlobeInner() {
  const globeRef = useRef<GlobeHandle | null>(null);
  // Mirror the imperative-handle ref into state so the init effect can react
  // to it. react-kapsule fires onGlobeReady from its mount layoutEffect, which
  // can run *before* useImperativeHandle commits the parent ref — depending
  // only on globeReady would then run the effect once with a null ref and
  // never retry once the ref shows up.
  const [globeHandle, setGlobeHandle] = useState<GlobeHandle | null>(null);
  const setGlobeRef = useCallback((handle: GlobeHandle | null) => {
    globeRef.current = handle;
    setGlobeHandle(handle);
  }, []);
  const from = useSwapStore((s) => s.from);
  const to = useSwapStore((s) => s.to);
  const setToken = useSwapStore((s) => s.setToken);

  const containerRef = useRef<HTMLDivElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 480, height: 480 });
  const [clickContext, setClickContext] = useState<ClickContext | null>(null);
  const [spinning, setSpinning] = useState(true);
  const [showFlags, setShowFlags] = useState(false);
  const [altitude, setAltitude] = useState(DEFAULT_POV.altitude);
  const [globeReady, setGlobeReady] = useState(false);

  // Three-bucket label size. The labels layer only rebuilds when crossing
  // a bucket boundary (a couple of times across a full zoom, not per
  // frame), so this stays non-glitchy while keeping text readable at every
  // zoom level.
  const labelSize = useMemo(() => {
    if (altitude < 0.3) return 0.08;
    if (altitude < 0.8) return 0.32;
    return 1.4;
  }, [altitude]);

  // Mirrors the labelSize buckets so flag emoji scale roughly in step with
  // the text labels they replace.
  const flagFontPx = useMemo(() => {
    if (altitude < 0.3) return 14;
    if (altitude < 0.8) return 20;
    return 28;
  }, [altitude]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const measure = () => {
      const w = el.clientWidth || 480;
      setSize({ width: w, height: Math.min(Math.max(w, 360), 560) });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (!clickContext) return;
    const onDown = (e: MouseEvent) => {
      if (popoverRef.current?.contains(e.target as Node)) return;
      setClickContext(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setClickContext(null);
    };
    const t = setTimeout(() => {
      document.addEventListener("mousedown", onDown);
      document.addEventListener("keydown", onKey);
    }, 0);
    return () => {
      clearTimeout(t);
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [clickContext]);

  const oceanMaterial = useMemo(
    () => new MeshBasicMaterial({ color: OCEAN_COLOR }),
    [],
  );

  // Drive init from an effect that depends on BOTH onGlobeReady having fired
  // and the imperative handle being committed. react-kapsule fires
  // onGlobeReady from its mount layoutEffect, which can run before
  // useImperativeHandle commits the parent ref. If we only keyed on
  // globeReady, the effect could run once with a null handle and never
  // retry once the handle showed up.
  const handleGlobeReady = useCallback(() => setGlobeReady(true), []);

  useEffect(() => {
    if (!globeReady || !globeHandle) return;
    const controls = globeHandle.controls() as {
      autoRotate: boolean;
      autoRotateSpeed: number;
      minDistance?: number;
      maxDistance?: number;
    };
    // Negative speed rotates the camera eastward, so starting over the US
    // gradually pans across the Atlantic to reveal Europe (USD → EUR).
    controls.autoRotateSpeed = -0.7;
    // Let the user dolly very close so dense regions (Caribbean, Eurozone)
    // become separable.
    controls.minDistance = 101;
    controls.maxDistance = 600;
    globeHandle.pointOfView(DEFAULT_POV, 0);

    const scene = globeHandle.scene();
    // Two layers — a dense bed of faint pinpricks plus a sparser layer of
    // brighter beacons — gives a natural twinkle-free starfield density.
    const layers = [
      makeStarLayer({ count: 2500, radius: 700, size: 1.1, opacity: 0.55 }),
      makeStarLayer({ count: 280, radius: 700, size: 2.2, opacity: 1 }),
    ];
    for (const layer of layers) scene.add(layer);
    return () => {
      for (const layer of layers) {
        scene.remove(layer);
        layer.geometry.dispose();
        (layer.material as PointsMaterial).dispose();
      }
    };
  }, [globeReady, globeHandle]);

  useEffect(() => {
    if (!globeReady || !globeHandle) return;
    const ctrl = globeHandle.controls();
    ctrl.autoRotate = spinning;
  }, [globeReady, globeHandle, spinning]);

  const resetView = () => {
    globeRef.current?.pointOfView(DEFAULT_POV, 800);
  };

  useAppEvent("resetGlobe", () => resetView());
  useAppEvent("toggleSpin", () => setSpinning((v) => !v));
  useAppEvent("toggleFlags", () => setShowFlags((v) => !v));

  const toggleSpin = () => setSpinning((s) => !s);

  const ZOOM_STEP = 1.3;
  const MIN_ALT = 0.05;
  const MAX_ALT = 4.5;
  const zoom = (factor: number) => {
    const g = globeRef.current;
    if (!g) return;
    const cur = g.pointOfView();
    const altitude = Math.max(
      MIN_ALT,
      Math.min(MAX_ALT, cur.altitude * factor),
    );
    g.pointOfView({ lat: cur.lat, lng: cur.lng, altitude }, 250);
  };
  const zoomIn = () => zoom(1 / ZOOM_STEP);
  const zoomOut = () => zoom(ZOOM_STEP);

  const focusOnArc = () => {
    const start = findPin(from.cca2);
    const end = findPin(to.cca2);
    if (!start || !end || !globeRef.current) return;
    const toRad = (d: number) => (d * Math.PI) / 180;
    const toDeg = (r: number) => (r * 180) / Math.PI;
    const phi1 = toRad(start.lat);
    const phi2 = toRad(end.lat);
    const lam1 = toRad(start.lng);
    const lam2 = toRad(end.lng);
    const Bx = Math.cos(phi2) * Math.cos(lam2 - lam1);
    const By = Math.cos(phi2) * Math.sin(lam2 - lam1);
    const midPhi = Math.atan2(
      Math.sin(phi1) + Math.sin(phi2),
      Math.sqrt((Math.cos(phi1) + Bx) ** 2 + By ** 2),
    );
    const midLam = lam1 + Math.atan2(By, Math.cos(phi1) + Bx);
    // Great-circle angular distance (haversine) — used to pick an altitude
    // that keeps both endpoints comfortably in frame.
    const dPhi = phi2 - phi1;
    const dLam = lam2 - lam1;
    const a =
      Math.sin(dPhi / 2) ** 2 +
      Math.cos(phi1) * Math.cos(phi2) * Math.sin(dLam / 2) ** 2;
    const angular = 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
    const altitude = Math.max(1.4, Math.min(3.0, 1.2 + angular * 0.9));
    globeRef.current.pointOfView(
      { lat: toDeg(midPhi), lng: toDeg(midLam), altitude },
      800,
    );
  };

  useAppEvent("focusRoute", () => focusOnArc());

  const arcs = useMemo(() => {
    const start = findPin(from.cca2);
    const end = findPin(to.cca2);
    if (!start || !end) return [];
    // Scale apex altitude with great-circle distance so widely-separated
    // (especially near-antipodal) pairs like JPY ↔ BRL arch high enough to
    // clear the globe instead of clipping through it.
    const toRad = (d: number) => (d * Math.PI) / 180;
    const phi1 = toRad(start.lat);
    const phi2 = toRad(end.lat);
    const dPhi = phi2 - phi1;
    const dLam = toRad(end.lng - start.lng);
    const h =
      Math.sin(dPhi / 2) ** 2 +
      Math.cos(phi1) * Math.cos(phi2) * Math.sin(dLam / 2) ** 2;
    const angular = 2 * Math.atan2(Math.sqrt(h), Math.sqrt(1 - h));
    const altitude = 0.15 + (angular / Math.PI) * 0.4;
    return [
      {
        startLat: start.lat,
        startLng: start.lng,
        endLat: end.lat,
        endLng: end.lng,
        altitude,
      },
    ];
  }, [from.cca2, to.cca2]);

  const polygonCapColor = (d: object) => {
    const f = d as CountryFeature;
    const supports = f.properties.currencies;
    if (supports.length === 0) return LAND_UNCOVERED;
    if (supports.includes(from.currency)) return SELL_TINT;
    if (supports.includes(to.currency)) return BUY_TINT;
    return LAND_COVERED;
  };

  const polygonAltitude = (d: object) => {
    const f = d as CountryFeature;
    const supports = f.properties.currencies;
    if (supports.length === 0) return 0.003;
    if (supports.includes(from.currency) || supports.includes(to.currency)) {
      return 0.008;
    }
    return 0.005;
  };

  const openPickerAt = useCallback(
    (
      name: string,
      cca2: string,
      currencies: IsoCurrencyCode[],
      clientX: number,
      clientY: number,
    ) => {
      if (currencies.length === 0 || !cca2) {
        setClickContext(null);
        return;
      }
      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const localX = clientX - rect.left;
      const localY = clientY - rect.top;
      const x = Math.min(Math.max(8, localX), rect.width - POPOVER_WIDTH - 8);
      const y = Math.min(
        Math.max(8, localY + 8),
        rect.height - POPOVER_MAX_HEIGHT - 8,
      );
      setClickContext({ countryName: name, cca2, currencies, x, y });
    },
    [],
  );

  const onPolygonClick = (poly: object, event: MouseEvent) => {
    setSpinning(false);
    const f = poly as CountryFeature;
    openPickerAt(
      f.properties.name,
      f.properties.cca2,
      f.properties.currencies,
      event.clientX,
      event.clientY,
    );
  };

  const onLabelClick = (label: object, event: MouseEvent) => {
    setSpinning(false);
    const p = label as CountryPin;
    openPickerAt(p.name, p.cca2, [p.currency], event.clientX, event.clientY);
  };

  const onGlobeClick = () => setSpinning(false);

  // Pause on drag (rotate) but not on wheel/pinch zoom.
  //   - Mouse: pointermove with primary button held === dragging.
  //   - Touch: pointermove with exactly one active pointer === single-finger
  //     rotate; two pointers === pinch, which we want to ignore.
  //   - Wheel: doesn't fire pointer events, so it's naturally exempt.
  const activePointers = useRef<Set<number>>(new Set());
  const onPointerDown = (e: React.PointerEvent) => {
    activePointers.current.add(e.pointerId);
  };
  const onPointerUp = (e: React.PointerEvent) => {
    activePointers.current.delete(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    if (e.pointerType === "mouse") {
      if (e.buttons & 1) setSpinning(false);
      return;
    }
    if (activePointers.current.size === 1) setSpinning(false);
  };

  const applyToSide = (
    side: "from" | "to",
    currency: IsoCurrencyCode,
    symbol: string,
    cca2: string,
  ) => {
    setToken(side, currency, symbol, cca2);
    setClickContext(null);
  };

  return (
    <div
      ref={containerRef}
      onPointerDown={onPointerDown}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onPointerMove={onPointerMove}
      className="relative w-full overflow-hidden rounded-xl border border-border bg-[#020617]"
    >
      <Globe
        ref={setGlobeRef as never}
        width={size.width}
        height={size.height}
        backgroundColor="#020617"
        globeMaterial={oceanMaterial}
        showAtmosphere={true}
        atmosphereColor="#7dd3fc"
        atmosphereAltitude={0.18}
        onGlobeReady={handleGlobeReady}
        polygonsData={WORLD_POLYGONS}
        polygonAltitude={polygonAltitude}
        polygonCapColor={polygonCapColor}
        polygonSideColor={() => "rgba(0,0,0,0.2)"}
        polygonStrokeColor={() => "rgba(255,255,255,0.18)"}
        polygonLabel={(d: object) =>
          `<div style="font-family: var(--font-geist-sans); font-size: 12px; padding: 4px 8px; background: rgba(0,0,0,0.7); border-radius: 4px; color: white;">${(d as CountryFeature).properties.name}</div>`
        }
        onPolygonClick={onPolygonClick}
        onGlobeClick={onGlobeClick}
        ringsData={COUNTRY_PINS}
        ringLat={(d: object) => (d as CountryPin).lat}
        ringLng={(d: object) => (d as CountryPin).lng}
        ringColor={() => "rgba(167, 243, 208, 0.45)"}
        ringMaxRadius={1.6}
        ringPropagationSpeed={0.7}
        ringRepeatPeriod={2200}
        ringAltitude={0.012}
        arcsData={arcs}
        arcStartLat={(d: object) => (d as { startLat: number }).startLat}
        arcStartLng={(d: object) => (d as { startLng: number }).startLng}
        arcStartAltitude={0.012}
        arcEndLat={(d: object) => (d as { endLat: number }).endLat}
        arcEndLng={(d: object) => (d as { endLng: number }).endLng}
        arcEndAltitude={0.012}
        arcColor={() => ARC_COLOR}
        arcStroke={0.8}
        arcDashLength={0.4}
        arcDashGap={0.2}
        arcDashAnimateTime={2000}
        // Apex altitude scales with great-circle distance (see useMemo above):
        // close pairs get a low bow, near-antipodal pairs arch high enough to
        // clear the globe surface instead of clipping through it.
        arcAltitude={(d: object) => (d as { altitude: number }).altitude}
        labelsData={
          showFlags || altitude >= LABEL_VISIBILITY_ALTITUDE
            ? []
            : altitude < 0.8
              ? COUNTRY_PINS
              : FAR_ZOOM_PINS
        }
        labelLat={(d: object) => (d as CountryPin).lat}
        labelLng={(d: object) => (d as CountryPin).lng}
        labelText={(d: object) =>
          // globe.gl uses the default Helvetiker typeface for labels, which
          // doesn't include Latin Extended glyphs (é, ô, ç, etc.). Strip
          // diacritics so names like "Saint Barthélemy" render as
          // "Saint Barthelemy" instead of "Saint Barth?lemy".
          (d as CountryPin).name
            .normalize("NFKD")
            .replace(/\p{Diacritic}/gu, "")
        }
        labelSize={labelSize}
        labelDotRadius={labelSize * 0.36}
        labelAltitude={0.012}
        labelColor={() => "rgba(241, 245, 249, 0.95)"}
        labelResolution={2}
        labelIncludeDot={true}
        onLabelClick={onLabelClick}
        htmlElementsData={
          showFlags && altitude < LABEL_VISIBILITY_ALTITUDE
            ? altitude < 0.8
              ? COUNTRY_PINS
              : FAR_ZOOM_PINS
            : []
        }
        htmlLat={(d: object) => (d as CountryPin).lat}
        htmlLng={(d: object) => (d as CountryPin).lng}
        htmlAltitude={0.012}
        htmlElement={(d: object) => {
          const pin = d as CountryPin;
          const el = document.createElement("div");
          el.textContent = cca2ToFlag(pin.cca2);
          el.style.fontSize = `${flagFontPx}px`;
          el.style.lineHeight = "1";
          el.style.cursor = "pointer";
          el.style.userSelect = "none";
          el.style.transform = "translate(-50%, -50%)";
          el.title = pin.name;
          el.addEventListener("click", (e) => {
            onLabelClick(pin, e);
          });
          return el;
        }}
        onZoom={(pov: { altitude: number }) => setAltitude(pov.altitude)}
      />

      <div className="absolute top-3 right-3 z-20 flex flex-col gap-2">
        <button
          type="button"
          onClick={resetView}
          title="Reset view"
          className="flex h-9 w-9 items-center justify-center rounded-full border border-border bg-background/80 text-muted-fg shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent"
          aria-label="Reset globe orientation"
        >
          <Compass size={16} />
        </button>
        <button
          type="button"
          onClick={focusOnArc}
          title="Focus on swap route"
          className="flex h-9 w-9 items-center justify-center rounded-full border border-border bg-background/80 text-muted-fg shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent"
          aria-label="Focus globe on swap route"
        >
          <Crosshair size={16} />
        </button>
        <button
          type="button"
          onClick={() => setShowFlags((v) => !v)}
          title={showFlags ? "Show country names" : "Show flag emojis"}
          className={`flex h-9 w-9 items-center justify-center rounded-full border bg-background/80 shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent ${
            showFlags
              ? "border-accent text-accent"
              : "border-border text-muted-fg"
          }`}
          aria-label={
            showFlags
              ? "Switch globe labels to country names"
              : "Switch globe labels to flag emojis"
          }
          aria-pressed={showFlags}
        >
          <Flag size={16} />
        </button>
        <button
          type="button"
          onClick={toggleSpin}
          title={spinning ? "Pause rotation" : "Spin globe"}
          className="flex h-9 w-9 items-center justify-center rounded-full border border-border bg-background/80 text-muted-fg shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent"
          aria-label={
            spinning ? "Pause globe rotation" : "Start globe rotation"
          }
        >
          {spinning ? (
            <Pause size={16} />
          ) : (
            <Play size={16} className="translate-x-px" />
          )}
        </button>
        <button
          type="button"
          onClick={zoomIn}
          title="Zoom in"
          className="flex h-9 w-9 items-center justify-center rounded-full border border-border bg-background/80 text-muted-fg shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
          disabled={altitude <= MIN_ALT + 0.01}
          aria-label="Zoom in"
        >
          <Plus size={16} />
        </button>
        <button
          type="button"
          onClick={zoomOut}
          title="Zoom out"
          className="flex h-9 w-9 items-center justify-center rounded-full border border-border bg-background/80 text-muted-fg shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
          disabled={altitude >= MAX_ALT - 0.01}
          aria-label="Zoom out"
        >
          <Minus size={16} />
        </button>
      </div>

      {clickContext && (
        <div
          ref={popoverRef}
          className="absolute z-30 flex flex-col overflow-hidden rounded-xl border border-border bg-background shadow-lg"
          style={{
            left: clickContext.x,
            top: clickContext.y,
            width: POPOVER_WIDTH,
            maxHeight: POPOVER_MAX_HEIGHT,
          }}
        >
          <div className="border-border border-b px-3 py-2 text-xs">
            <span className="truncate font-medium text-foreground">
              {clickContext.countryName}
            </span>
          </div>
          <div className="flex-1 overflow-y-auto p-1">
            {clickContext.currencies.map((cur) => (
              <div key={cur} className="py-1">
                <CurrencyGroupHeader code={cur} />
                {CURRENCIES[cur].stablecoins.map((s) => {
                  const isFromHere =
                    cur === from.currency && s.symbol === from.stablecoin;
                  const isToHere =
                    cur === to.currency && s.symbol === to.stablecoin;
                  return (
                    <div
                      key={`${cur}-${s.symbol}`}
                      className="flex w-full items-center gap-1 rounded-md px-2 py-1.5"
                    >
                      {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
                      <img
                        src={tokenIconUrl(s.symbol)}
                        alt=""
                        aria-hidden
                        width={20}
                        height={20}
                        className="h-5 w-5 shrink-0 rounded-full"
                      />
                      <span className="flex min-w-0 flex-1 flex-col text-sm">
                        <span className="font-mono text-foreground">
                          {s.symbol}
                        </span>
                        {s.name !== s.symbol && (
                          <span className="truncate text-muted-fg text-xs">
                            {s.name}
                          </span>
                        )}
                      </span>
                      <button
                        type="button"
                        disabled={isToHere}
                        onClick={() =>
                          applyToSide("from", cur, s.symbol, clickContext.cca2)
                        }
                        title={
                          isToHere
                            ? "Already selected as To"
                            : `Swap from ${s.symbol} (${clickContext.countryName})`
                        }
                        className={`w-14 shrink-0 rounded px-2 py-1 text-center font-medium text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                          isFromHere
                            ? "bg-[#3b82f6] text-white"
                            : "border border-border text-muted-fg hover:border-[#3b82f6] hover:text-[#3b82f6]"
                        }`}
                      >
                        From
                      </button>
                      <button
                        type="button"
                        disabled={isFromHere}
                        onClick={() =>
                          applyToSide("to", cur, s.symbol, clickContext.cca2)
                        }
                        title={
                          isFromHere
                            ? "Already selected as From"
                            : `Swap to ${s.symbol} (${clickContext.countryName})`
                        }
                        className={`w-14 shrink-0 rounded px-2 py-1 text-center font-medium text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                          isToHere
                            ? "bg-[#10b981] text-white"
                            : "border border-border text-muted-fg hover:border-[#10b981] hover:text-[#10b981]"
                        }`}
                      >
                        To
                      </button>
                    </div>
                  );
                })}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

export function GlobePanel() {
  return (
    <GlobeErrorBoundary>
      <GlobeInner />
    </GlobeErrorBoundary>
  );
}
