"use client";

import * as Dialog from "@radix-ui/react-dialog";
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
  Mesh,
  MeshBasicMaterial,
  type Object3D,
  Points,
  PointsMaterial,
  type Scene,
  TorusGeometry,
} from "three";
import { COUNTRY_PINS, type CountryPin, findPin } from "@/lib/countries";
import {
  CURRENCIES,
  flagUrl,
  type IsoCurrencyCode,
  tokenIconUrl,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { useSwapStore } from "@/lib/store";
import { type CountryFeature, WORLD_POLYGONS } from "@/lib/world-polygons";
import { CurrencyGroupHeader } from "./CurrencyGroupHeader";
import { Compass, Crosshair, Flag, Minus, Pause, Play, Plus, X } from "./icons";
import { TokenInfoLink } from "./TokenInfoLink";
import { TokenMintActions } from "./TokenMintActions";

const Globe = dynamic(() => import("react-globe.gl"), {
  ssr: false,
  loading: () => (
    <div className="flex h-[400px] w-full items-center justify-center text-muted-fg text-sm">
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

// Height of the "same-country" pillar (a vertical cylinder over the shared
// anchor) and the spinning ring that sits on top of it.
const PILLAR_ALTITUDE = 0.18;
// three-globe's default sphere radius. react-globe.gl's typings don't expose
// the runtime globeRadius arg in customThreeObject callbacks, but it equals
// this constant unless overridden — which we don't.
const GLOBE_RADIUS = 100;

// Start the view centered roughly over the eastern US so the auto-rotation
// reveals the Atlantic and then Europe — the canonical USD → EUR path.
const DEFAULT_POV = { lat: 30, lng: -75, altitude: 1.5 };

// Below this altitude, country-name labels become visible.
const LABEL_VISIBILITY_ALTITUDE = 2.6;

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
        <div className="flex h-[400px] w-full flex-col items-center justify-center gap-2 p-4 text-center text-muted-fg text-sm">
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
  const [size, setSize] = useState({ width: 480, height: 480 });
  const [clickContext, setClickContext] = useState<ClickContext | null>(null);
  // Top edge of the globe in viewport coordinates, captured when the picker
  // opens so the dialog renders flush with the top of the map rather than the
  // center of the viewport.
  const [pickerTop, setPickerTop] = useState<number | null>(null);
  useEffect(() => {
    if (clickContext === null) return;
    const top = containerRef.current?.getBoundingClientRect().top;
    if (typeof top === "number") setPickerTop(top);
  }, [clickContext]);
  const [spinning, setSpinning] = useState(true);
  const [showFlags, setShowFlags] = useState(false);
  const [altitude, setAltitude] = useState(DEFAULT_POV.altitude);
  const [globeReady, setGlobeReady] = useState(false);
  // Gate visibility so the user never sees the lib's default POV before our
  // pointOfView snap commits. Flipped after the snap inside the setup effect.
  const [revealed, setRevealed] = useState(false);
  // Ref to the spinning-ring mesh so a RAF loop can mutate its rotation
  // each frame without going through React state.
  const ringRef = useRef<Mesh | null>(null);
  const [flashOn, setFlashOn] = useState(false);

  const sameCca2 = from.cca2 === to.cca2;
  const sameToken =
    from.currency === to.currency && from.stablecoin === to.stablecoin;

  // 500ms polygon-cap flash for the fully-degenerate sameToken case to
  // mirror the disabled Swap button. Same-country / different-stable swaps
  // get the spinning ring on top of the pillar instead (see customLayer).
  useEffect(() => {
    if (!sameToken) {
      setFlashOn(false);
      return;
    }
    const id = setInterval(() => setFlashOn((v) => !v), 500);
    return () => clearInterval(id);
  }, [sameToken]);

  // Spin the ring continuously while it's visible by directly mutating the
  // mesh's rotation in a RAF loop — avoids re-rendering React state each
  // frame just to update three.js scene transforms.
  useEffect(() => {
    if (!sameCca2 || sameToken) return;
    let raf = 0;
    const tick = () => {
      const m = ringRef.current;
      if (m) m.rotateZ(0.03);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [sameCca2, sameToken]);

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
      setSize({ width: w, height: Math.min(Math.max(w * 0.85, 320), 480) });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

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
    // react-globe.gl's pointOfView still runs an internal d3 transition even
    // with duration=0, so the camera glides into DEFAULT_POV.lat over a few
    // frames. Keep the canvas hidden until the latitude has actually settled
    // there, with a hard fallback so a stuck poll doesn't leave it invisible.
    // (lng isn't compared — autoRotate is already moving it.)
    let revealRaf = 0;
    const checkSettled = () => {
      const pov = globeHandle.pointOfView();
      if (Math.abs(pov.lat - DEFAULT_POV.lat) < 0.5) {
        setRevealed(true);
        return;
      }
      revealRaf = requestAnimationFrame(checkSettled);
    };
    revealRaf = requestAnimationFrame(checkSettled);
    const revealFallback = window.setTimeout(() => setRevealed(true), 1500);

    const scene = globeHandle.scene();
    // Two layers — a dense bed of faint pinpricks plus a sparser layer of
    // brighter beacons — gives a natural twinkle-free starfield density.
    const layers = [
      makeStarLayer({ count: 2500, radius: 700, size: 1.1, opacity: 0.55 }),
      makeStarLayer({ count: 280, radius: 700, size: 2.2, opacity: 1 }),
    ];
    for (const layer of layers) scene.add(layer);
    return () => {
      cancelAnimationFrame(revealRaf);
      window.clearTimeout(revealFallback);
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

  useAppEvent("zoomIn", () => zoomIn());
  useAppEvent("zoomOut", () => zoomOut());

  // Arrow-key panning. Step shrinks with altitude so fine adjustments are
  // possible when zoomed in. Lat is clamped to ±85° to avoid the
  // OrbitControls polar-flip near the poles.
  useAppEvent("pan", (dir) => {
    const g = globeRef.current;
    if (!g) return;
    const cur = g.pointOfView();
    const step = Math.max(2, Math.min(20, cur.altitude * 10));
    const next: Pov = { ...cur };
    if (dir === "up") next.lat = Math.min(85, cur.lat + step);
    else if (dir === "down") next.lat = Math.max(-85, cur.lat - step);
    else if (dir === "left") next.lng = cur.lng - step;
    else if (dir === "right") next.lng = cur.lng + step;
    g.pointOfView(next, 250);
    setSpinning(false);
  });

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
    // Hide the arc whenever both anchors collide — the polygon flash (same
    // token) or the pillar (same country, different stable) takes over.
    if (sameCca2) return [];
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
  }, [from.cca2, to.cca2, sameCca2]);

  // Pillar that flashes over the shared anchor when both sides share a
  // country but use different stables (a valid swap that's just visually
  // confusing with a regular arc).
  const pillarPins = useMemo(() => {
    if (!sameCca2 || sameToken) return [];
    const pin = findPin(from.cca2);
    return pin ? [pin] : [];
  }, [sameCca2, sameToken, from.cca2]);

  const polygonCapColor = (d: object) => {
    const f = d as CountryFeature;
    const supports = f.properties.currencies;
    if (supports.length === 0) return LAND_UNCOVERED;
    if (sameToken && f.properties.cca2 === from.cca2) {
      return flashOn ? BUY_TINT : SELL_TINT;
    }
    if (supports.includes(from.currency)) return SELL_TINT;
    if (supports.includes(to.currency)) return BUY_TINT;
    return LAND_COVERED;
  };

  const polygonAltitude = (d: object) => {
    const f = d as CountryFeature;
    const supports = f.properties.currencies;
    // Large polygons (Greenland, Antarctica) still flicker near the globe
    // shell at very low altitude — keep everything well above the atmosphere
    // shader. Overlay layers (rings/labels/arcs) sit above at 0.018.
    if (supports.length === 0) return 0.008;
    if (supports.includes(from.currency) || supports.includes(to.currency)) {
      return 0.013;
    }
    return 0.011;
  };

  const closePicker = useCallback(() => {
    setClickContext(null);
    setSpinning(true);
  }, []);

  const openPickerAt = useCallback(
    (name: string, cca2: string, currencies: IsoCurrencyCode[]) => {
      if (currencies.length === 0 || !cca2) {
        closePicker();
        return;
      }
      setClickContext({ countryName: name, cca2, currencies });
    },
    [closePicker],
  );

  const onPolygonClick = (poly: object) => {
    setSpinning(false);
    const f = poly as CountryFeature;
    openPickerAt(f.properties.name, f.properties.cca2, f.properties.currencies);
  };

  const onLabelClick = (label: object) => {
    setSpinning(false);
    const p = label as CountryPin;
    openPickerAt(p.name, p.cca2, [p.currency]);
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
    closePicker();
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
      <div
        className={`transition-opacity duration-0 ${revealed ? "opacity-100" : "opacity-0"}`}
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
          ringAltitude={0.018}
          pointsData={pillarPins}
          pointLat={(d: object) => (d as CountryPin).lat}
          pointLng={(d: object) => (d as CountryPin).lng}
          pointAltitude={PILLAR_ALTITUDE}
          pointRadius={0.35}
          pointResolution={12}
          pointColor={() => BUY_TINT}
          customLayerData={pillarPins}
          customThreeObject={() => {
            // Partial torus (3/4 of a full ring) so the spin is visibly
            // rotational instead of a uniform disc.
            const geom = new TorusGeometry(
              GLOBE_RADIUS * 0.04,
              GLOBE_RADIUS * 0.006,
              8,
              48,
              Math.PI * 1.5,
            );
            const mat = new MeshBasicMaterial({ color: BUY_TINT });
            const mesh = new Mesh(geom, mat);
            ringRef.current = mesh;
            return mesh;
          }}
          customThreeObjectUpdate={(obj: Object3D, d: object) => {
            // Place the ring at the pillar's tip and orient its plane tangent
            // to the globe surface (TorusGeometry's ring axis is +Z by default;
            // lookAt(origin) makes +Z point outward, so the ring lies flat).
            const pin = d as CountryPin;
            const phi = ((90 - pin.lat) * Math.PI) / 180;
            const theta = ((pin.lng + 90) * Math.PI) / 180;
            const r = GLOBE_RADIUS * (1 + PILLAR_ALTITUDE);
            obj.position.set(
              -r * Math.sin(phi) * Math.cos(theta),
              r * Math.cos(phi),
              r * Math.sin(phi) * Math.sin(theta),
            );
            obj.lookAt(0, 0, 0);
          }}
          arcsData={arcs}
          arcStartLat={(d: object) => (d as { startLat: number }).startLat}
          arcStartLng={(d: object) => (d as { startLng: number }).startLng}
          arcStartAltitude={0.018}
          arcEndLat={(d: object) => (d as { endLat: number }).endLat}
          arcEndLng={(d: object) => (d as { endLng: number }).endLng}
          arcEndAltitude={0.018}
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
          labelAltitude={0.018}
          labelColor={() => "rgba(241, 245, 249, 0.95)"}
          labelResolution={2}
          labelIncludeDot={true}
          onLabelClick={onLabelClick}
          htmlElementsData={
            showFlags ? (altitude < 0.8 ? COUNTRY_PINS : FAR_ZOOM_PINS) : []
          }
          htmlLat={(d: object) => (d as CountryPin).lat}
          htmlLng={(d: object) => (d as CountryPin).lng}
          htmlAltitude={0.018}
          htmlElement={(d: object) => {
            const pin = d as CountryPin;
            const el = document.createElement("img");
            el.src = flagUrl(pin.cca2);
            el.alt = pin.name;
            // Twemoji SVGs are square — pin size tracks the label-size bucket
            // so flag pins remain in step with the country-name labels they
            // replace.
            el.width = flagFontPx;
            el.height = flagFontPx;
            el.draggable = false;
            el.style.cursor = "pointer";
            el.style.userSelect = "none";
            el.style.transform = "translate(-50%, -50%)";
            // Lift the glyph off bright ocean/land without boxing the
            // bounding square — drop-shadow respects the rounded-rect shape
            // of Twemoji's flag artwork.
            el.style.filter = "drop-shadow(0 1px 1px rgba(0,0,0,0.5))";
            // globe.gl's HTML overlay container sets pointer-events: none so
            // canvas interactions (rotate/zoom) still work through it; each
            // child has to opt back in to receive its own click.
            el.style.pointerEvents = "auto";
            // Keep flags below the country popover (z-40) when both are visible.
            el.style.zIndex = "1";
            el.title = pin.name;
            el.addEventListener("click", () => {
              onLabelClick(pin);
            });
            return el;
          }}
          onZoom={(pov: { altitude: number }) => setAltitude(pov.altitude)}
        />
      </div>

      <div className="absolute top-3 left-3 z-20 flex flex-col gap-2">
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
      </div>

      <div className="absolute top-3 right-3 z-20 flex flex-col gap-2">
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

      <Dialog.Root
        open={clickContext !== null}
        onOpenChange={(o) => {
          if (!o) closePicker();
        }}
      >
        <Dialog.Portal>
          <Dialog.Overlay className="fixed inset-0 z-[60] bg-background/80 backdrop-blur-2xl" />
          <Dialog.Content
            aria-describedby={undefined}
            style={
              pickerTop !== null
                ? {
                    top: pickerTop,
                    maxHeight: `calc(100vh - ${pickerTop}px - 1rem)`,
                  }
                : undefined
            }
            className={`-translate-x-1/2 fixed left-1/2 z-[70] flex w-[min(420px,calc(100vw-2rem))] flex-col overflow-hidden rounded-xl border border-border bg-background shadow-lg ${
              pickerTop === null
                ? "-translate-y-1/2 top-1/2 max-h-[calc(100vh-3rem)]"
                : ""
            }`}
          >
            <div className="flex items-center gap-2 border-border border-b px-3 py-2">
              <Dialog.Title className="min-w-0 flex-1 truncate text-foreground text-sm">
                {clickContext?.countryName ?? ""}
              </Dialog.Title>
              <kbd className="hidden shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-fg sm:inline-block">
                Esc
              </kbd>
              <Dialog.Close
                aria-label="Close"
                className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-fg hover:bg-muted hover:text-foreground"
              >
                <X size={14} />
              </Dialog.Close>
            </div>
            <div className="flex-1 overflow-y-auto p-1">
              {clickContext?.currencies.map((cur) => (
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
                            applyToSide(
                              "from",
                              cur,
                              s.symbol,
                              clickContext.cca2,
                            )
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
                        <TokenMintActions symbol={s.symbol} mint={s.mint} />
                        <TokenInfoLink symbol={s.symbol} />
                      </div>
                    );
                  })}
                </div>
              ))}
            </div>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
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
