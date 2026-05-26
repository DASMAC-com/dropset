"use client";

import dynamic from "next/dynamic";
import {
  Component,
  type ComponentProps,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Mesh,
  MeshBasicMaterial,
  type Object3D,
  type Scene,
  TorusGeometry,
} from "three";
import {
  Compass,
  Crosshair,
  Flag,
  Minus,
  Pause,
  Play,
  Plus,
} from "@/components/icons";
import {
  type ClickContext,
  CountryPickerDialog,
} from "@/components/picker/CountryPickerDialog";
import { COUNTRY_PINS, type CountryPin, findPin } from "@/lib/data/countries";
import { flagUrl, type IsoCurrencyCode } from "@/lib/data/currencies";
import { useAppEvent } from "@/lib/events";
import {
  ARC_COLOR,
  ARC_DASH_ANIM_MS,
  ARC_DASH_GAP,
  ARC_DASH_LENGTH,
  ARC_STROKE,
  ATMOSPHERE_ALTITUDE,
  ATMOSPHERE_COLOR,
  AUTO_ROTATE_SPEED,
  BACKGROUND_COLOR,
  BUY_TINT,
  CAMERA_MAX_DISTANCE,
  CAMERA_MIN_DISTANCE,
  DEFAULT_POV,
  FAR_ZOOM_ALTITUDE_THRESHOLD,
  FAR_ZOOM_PROXIMITY_DEG,
  FLAG_FONT_PX_CLOSE,
  FLAG_FONT_PX_FAR,
  FLAG_FONT_PX_MID,
  FOCUS_ARC_ANIMATION_MS,
  GLOBE_DEFAULT_WIDTH,
  GLOBE_HEIGHT_RATIO,
  GLOBE_MAX_HEIGHT,
  GLOBE_MIN_HEIGHT,
  GLOBE_RADIUS,
  LABEL_BUCKET_CLOSE_MAX,
  LABEL_BUCKET_MID_MAX,
  LABEL_COLOR,
  LABEL_DOT_RADIUS_FRAC,
  LABEL_RESOLUTION,
  LABEL_SIZE_CLOSE,
  LABEL_SIZE_FAR,
  LABEL_SIZE_MID,
  LABEL_VISIBILITY_ALTITUDE,
  LAND_COVERED,
  LAND_UNCOVERED,
  LAT_CLAMP_DEG,
  MAX_ALTITUDE,
  MIN_ALTITUDE,
  OCEAN_COLOR,
  OVERLAY_ALTITUDE,
  PAN_ANIMATION_MS,
  PAN_STEP_ALT_FACTOR,
  PAN_STEP_MAX_DEG,
  PAN_STEP_MIN_DEG,
  PILLAR_ALTITUDE,
  POLY_ALT_DEFAULT,
  POLY_ALT_HIGHLIGHTED,
  POLY_ALT_UNSUPPORTED,
  POLY_SIDE_COLOR,
  POLY_STROKE_COLOR,
  POV_SETTLE_TOLERANCE_DEG,
  RESET_VIEW_ANIMATION_MS,
  REVEAL_FALLBACK_MS,
  RING_COLOR,
  RING_MAX_RADIUS,
  RING_PROPAGATION_SPEED,
  RING_REPEAT_PERIOD_MS,
  RING_SPIN_PER_FRAME,
  RING_SWEEP_RAD,
  RING_TORUS_RAD_SEG,
  RING_TORUS_RADIUS_FRAC,
  RING_TORUS_TUB_SEG,
  RING_TORUS_TUBE_FRAC,
  SAME_TOKEN_FLASH_MS,
  SELL_TINT,
  ZOOM_STEP,
} from "@/lib/globe/globeConstants";
import {
  angularDistanceDeg,
  angularDistanceRad,
  arcApexAltitude,
  focusArcAltitude,
  greatCircleMidpoint,
  latLngToXYZ,
} from "@/lib/globe/globeMath";
import { installStarLayers } from "@/lib/globe/globeScene";
import {
  type CountryFeature,
  WORLD_POLYGONS,
} from "@/lib/globe/world-polygons";
import { useSwapStore, useSwapStoreApi } from "@/lib/store";
import { useSwapNav } from "@/lib/ui/swapUrl";

const Globe = dynamic(() => import("react-globe.gl"), {
  ssr: false,
  loading: () => (
    <div className="flex h-[400px] w-full items-center justify-center text-muted-fg text-sm">
      Loading globe…
    </div>
  ),
});

type Pov = { lat: number; lng: number; altitude: number };
type GlobeHandle = {
  controls: () => {
    autoRotate: boolean;
    autoRotateSpeed: number;
    minDistance?: number;
    maxDistance?: number;
  };
  pointOfView: (pov?: Pov, durationMs?: number) => Pov;
  scene: () => Scene;
};

// Empty pin array reused whenever labels or flags are hidden — keeps
// react-globe.gl from seeing a fresh `[]` reference per render and
// rebuilding its label/HTML layers unnecessarily.
const EMPTY_PINS: CountryPin[] = [];

// Pre-clustered subset of COUNTRY_PINS for the most zoomed-out
// label-visible altitude band: greedy area-weighted clustering drops the
// smaller pins inside any FAR_ZOOM_PROXIMITY_DEG-radius cluster so dense
// regions (Lesser Antilles, Eurozone microstates) don't pile labels on top
// of each other at the default view.
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

// Globe's ref prop is typed somewhat loosely upstream; narrow the cast to
// the shape we actually use (controls/pointOfView/scene) and only here.
type GlobeRefProp = ComponentProps<typeof Globe>["ref"];

function GlobeInner() {
  const globeRef = useRef<GlobeHandle | null>(null);
  // Mirror the imperative-handle ref into state so the init effect can
  // react to it. react-kapsule fires onGlobeReady from its mount
  // layoutEffect, which can run *before* useImperativeHandle commits the
  // parent ref — depending only on globeReady would then run the effect
  // once with a null ref and never retry once the ref showed up.
  const [globeHandle, setGlobeHandle] = useState<GlobeHandle | null>(null);
  const setGlobeRef = useCallback((handle: GlobeHandle | null) => {
    globeRef.current = handle;
    setGlobeHandle(handle);
  }, []);
  // Subscribe to primitive fields rather than `s.from` / `s.to` whole
  // SideState objects. Selector identity drives re-renders; returning a
  // fresh object each notification would re-render this panel on every
  // unrelated store mutation (amount typing, slippage toggle, etc.).
  const fromCca2 = useSwapStore((s) => s.from.cca2);
  const fromCurrency = useSwapStore((s) => s.from.currency);
  const fromStablecoin = useSwapStore((s) => s.from.stablecoin);
  const toCca2 = useSwapStore((s) => s.to.cca2);
  const toCurrency = useSwapStore((s) => s.to.currency);
  const toStablecoin = useSwapStore((s) => s.to.stablecoin);
  const store = useSwapStoreApi();
  const gotoSwap = useSwapNav();

  const containerRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({
    width: GLOBE_DEFAULT_WIDTH,
    height: GLOBE_MAX_HEIGHT,
  });
  const [clickContext, setClickContext] = useState<ClickContext | null>(null);
  // Top edge of the globe in viewport coordinates, captured when the picker
  // opens so the dialog renders flush with the top of the map rather than
  // the center of the viewport.
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
  const [revealed, setRevealed] = useState(false);
  const ringRef = useRef<Mesh | null>(null);
  const [flashOn, setFlashOn] = useState(false);

  const sameCca2 = fromCca2 === toCca2;
  const sameToken =
    fromCurrency === toCurrency && fromStablecoin === toStablecoin;

  // 500ms polygon-cap flash for the fully-degenerate sameToken case to
  // mirror the disabled Swap button. Same-country / different-stable swaps
  // get the spinning ring on top of the pillar instead (see customLayer).
  useEffect(() => {
    if (!sameToken) {
      setFlashOn(false);
      return;
    }
    const id = setInterval(() => setFlashOn((v) => !v), SAME_TOKEN_FLASH_MS);
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
      if (m) m.rotateZ(RING_SPIN_PER_FRAME);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [sameCca2, sameToken]);

  // Three-bucket label size — only rebuilds when crossing a bucket
  // boundary, not per frame.
  const labelSize = useMemo(() => {
    if (altitude < LABEL_BUCKET_CLOSE_MAX) return LABEL_SIZE_CLOSE;
    if (altitude < LABEL_BUCKET_MID_MAX) return LABEL_SIZE_MID;
    return LABEL_SIZE_FAR;
  }, [altitude]);
  const flagFontPx = useMemo(() => {
    if (altitude < LABEL_BUCKET_CLOSE_MAX) return FLAG_FONT_PX_CLOSE;
    if (altitude < LABEL_BUCKET_MID_MAX) return FLAG_FONT_PX_MID;
    return FLAG_FONT_PX_FAR;
  }, [altitude]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const measure = () => {
      const w = el.clientWidth || GLOBE_DEFAULT_WIDTH;
      setSize({
        width: w,
        height: Math.min(
          Math.max(w * GLOBE_HEIGHT_RATIO, GLOBE_MIN_HEIGHT),
          GLOBE_MAX_HEIGHT,
        ),
      });
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

  const handleGlobeReady = useCallback(() => setGlobeReady(true), []);

  useEffect(() => {
    if (!globeReady || !globeHandle) return;
    const controls = globeHandle.controls();
    controls.autoRotateSpeed = AUTO_ROTATE_SPEED;
    controls.minDistance = CAMERA_MIN_DISTANCE;
    controls.maxDistance = CAMERA_MAX_DISTANCE;
    globeHandle.pointOfView(DEFAULT_POV, 0);

    // react-globe.gl's pointOfView still runs an internal d3 transition
    // even with duration=0, so the camera glides into DEFAULT_POV.lat over
    // a few frames. Keep the canvas hidden until the latitude has
    // actually settled there, with a hard fallback so a stuck poll
    // doesn't leave it invisible.
    let revealRaf = 0;
    const checkSettled = () => {
      const pov = globeHandle.pointOfView();
      if (Math.abs(pov.lat - DEFAULT_POV.lat) < POV_SETTLE_TOLERANCE_DEG) {
        setRevealed(true);
        return;
      }
      revealRaf = requestAnimationFrame(checkSettled);
    };
    revealRaf = requestAnimationFrame(checkSettled);
    const revealFallback = window.setTimeout(
      () => setRevealed(true),
      REVEAL_FALLBACK_MS,
    );

    const disposeStars = installStarLayers(globeHandle.scene());

    return () => {
      cancelAnimationFrame(revealRaf);
      window.clearTimeout(revealFallback);
      disposeStars();
    };
  }, [globeReady, globeHandle]);

  useEffect(() => {
    if (!globeReady || !globeHandle) return;
    const ctrl = globeHandle.controls();
    ctrl.autoRotate = spinning;
  }, [globeReady, globeHandle, spinning]);

  const resetView = () => {
    globeRef.current?.pointOfView(DEFAULT_POV, RESET_VIEW_ANIMATION_MS);
  };

  useAppEvent("resetGlobe", () => resetView());
  useAppEvent("toggleSpin", () => setSpinning((v) => !v));
  useAppEvent("toggleFlags", () => setShowFlags((v) => !v));

  const toggleSpin = () => setSpinning((s) => !s);

  const zoom = (factor: number) => {
    const g = globeRef.current;
    if (!g) return;
    const cur = g.pointOfView();
    const newAltitude = Math.max(
      MIN_ALTITUDE,
      Math.min(MAX_ALTITUDE, cur.altitude * factor),
    );
    g.pointOfView(
      { lat: cur.lat, lng: cur.lng, altitude: newAltitude },
      PAN_ANIMATION_MS,
    );
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
    const step = Math.max(
      PAN_STEP_MIN_DEG,
      Math.min(PAN_STEP_MAX_DEG, cur.altitude * PAN_STEP_ALT_FACTOR),
    );
    const next: Pov = { ...cur };
    if (dir === "up") next.lat = Math.min(LAT_CLAMP_DEG, cur.lat + step);
    else if (dir === "down")
      next.lat = Math.max(-LAT_CLAMP_DEG, cur.lat - step);
    else if (dir === "left") next.lng = cur.lng - step;
    else if (dir === "right") next.lng = cur.lng + step;
    g.pointOfView(next, PAN_ANIMATION_MS);
    setSpinning(false);
  });

  const focusOnArc = () => {
    const start = findPin(fromCca2);
    const end = findPin(toCca2);
    if (!start || !end || !globeRef.current) return;
    const { lat, lng, angular } = greatCircleMidpoint(start, end);
    globeRef.current.pointOfView(
      { lat, lng, altitude: focusArcAltitude(angular) },
      FOCUS_ARC_ANIMATION_MS,
    );
  };

  useAppEvent("focusRoute", () => focusOnArc());

  const arcs = useMemo(() => {
    // Hide the arc whenever both anchors collide — the polygon flash (same
    // token) or the pillar (same country, different stable) takes over.
    if (sameCca2) return [];
    const start = findPin(fromCca2);
    const end = findPin(toCca2);
    if (!start || !end) return [];
    const angular = angularDistanceRad(start.lat, start.lng, end.lat, end.lng);
    return [
      {
        startLat: start.lat,
        startLng: start.lng,
        endLat: end.lat,
        endLng: end.lng,
        altitude: arcApexAltitude(angular),
      },
    ];
  }, [fromCca2, toCca2, sameCca2]);

  const pillarPins = useMemo(() => {
    if (!sameCca2 || sameToken) return [];
    const pin = findPin(fromCca2);
    return pin ? [pin] : [];
  }, [sameCca2, sameToken, fromCca2]);

  const polygonCapColor = (d: object) => {
    const f = d as CountryFeature;
    const supports = f.properties.currencies;
    if (supports.length === 0) return LAND_UNCOVERED;
    if (sameToken && f.properties.cca2 === fromCca2) {
      return flashOn ? BUY_TINT : SELL_TINT;
    }
    if (supports.includes(fromCurrency)) return SELL_TINT;
    if (supports.includes(toCurrency)) return BUY_TINT;
    return LAND_COVERED;
  };

  const polygonAltitude = (d: object) => {
    const f = d as CountryFeature;
    const supports = f.properties.currencies;
    if (supports.length === 0) return POLY_ALT_UNSUPPORTED;
    if (supports.includes(fromCurrency) || supports.includes(toCurrency)) {
      return POLY_ALT_HIGHLIGHTED;
    }
    return POLY_ALT_DEFAULT;
  };

  // Bucket the altitude into discrete tiers so the label / flag arrays
  // only change identity when the user crosses a meaningful threshold
  // (every couple of zooms), not on every onZoom tick. react-globe.gl
  // diffs these arrays by reference and rebuilds the layer when it
  // changes, so a memo that depends on the raw altitude would force a
  // rebuild on every frame of the d3 zoom transition.
  const labelBucket: "hidden" | "close" | "far" = showFlags
    ? "hidden"
    : altitude >= LABEL_VISIBILITY_ALTITUDE
      ? "hidden"
      : altitude < FAR_ZOOM_ALTITUDE_THRESHOLD
        ? "close"
        : "far";
  const flagBucket: "hidden" | "close" | "far" = !showFlags
    ? "hidden"
    : altitude < FAR_ZOOM_ALTITUDE_THRESHOLD
      ? "close"
      : "far";
  const labelsData = useMemo(() => {
    if (labelBucket === "hidden") return EMPTY_PINS;
    return labelBucket === "close" ? COUNTRY_PINS : FAR_ZOOM_PINS;
  }, [labelBucket]);
  const htmlElementsData = useMemo(() => {
    if (flagBucket === "hidden") return EMPTY_PINS;
    return flagBucket === "close" ? COUNTRY_PINS : FAR_ZOOM_PINS;
  }, [flagBucket]);

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

  // Pause on drag (rotate) but not on wheel/pinch zoom. Mouse: pointermove
  // with primary button held === dragging. Touch: pointermove with exactly
  // one active pointer === single-finger rotate; two pointers === pinch
  // (ignored). Wheel: doesn't fire pointer events, naturally exempt.
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
    store.getState().setToken(side, currency, symbol, cca2);
    const { from: f, to: t } = store.getState();
    gotoSwap(f.stablecoin, t.stablecoin);
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
          ref={setGlobeRef as unknown as GlobeRefProp}
          width={size.width}
          height={size.height}
          backgroundColor={BACKGROUND_COLOR}
          globeMaterial={oceanMaterial}
          showAtmosphere={true}
          atmosphereColor={ATMOSPHERE_COLOR}
          atmosphereAltitude={ATMOSPHERE_ALTITUDE}
          onGlobeReady={handleGlobeReady}
          polygonsData={WORLD_POLYGONS}
          polygonAltitude={polygonAltitude}
          polygonCapColor={polygonCapColor}
          polygonSideColor={() => POLY_SIDE_COLOR}
          polygonStrokeColor={() => POLY_STROKE_COLOR}
          polygonLabel={(d: object) =>
            `<div style="font-family: var(--font-geist-sans); font-size: 12px; padding: 4px 8px; background: rgba(0,0,0,0.7); border-radius: 4px; color: white;">${(d as CountryFeature).properties.name}</div>`
          }
          onPolygonClick={onPolygonClick}
          onGlobeClick={onGlobeClick}
          ringsData={COUNTRY_PINS}
          ringLat={(d: object) => (d as CountryPin).lat}
          ringLng={(d: object) => (d as CountryPin).lng}
          ringColor={() => RING_COLOR}
          ringMaxRadius={RING_MAX_RADIUS}
          ringPropagationSpeed={RING_PROPAGATION_SPEED}
          ringRepeatPeriod={RING_REPEAT_PERIOD_MS}
          ringAltitude={OVERLAY_ALTITUDE}
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
              GLOBE_RADIUS * RING_TORUS_RADIUS_FRAC,
              GLOBE_RADIUS * RING_TORUS_TUBE_FRAC,
              RING_TORUS_RAD_SEG,
              RING_TORUS_TUB_SEG,
              RING_SWEEP_RAD,
            );
            const mat = new MeshBasicMaterial({ color: BUY_TINT });
            const mesh = new Mesh(geom, mat);
            ringRef.current = mesh;
            return mesh;
          }}
          customThreeObjectUpdate={(obj: Object3D, d: object) => {
            // Place the ring at the pillar's tip and orient its plane
            // tangent to the globe surface (TorusGeometry's ring axis is
            // +Z by default; lookAt(origin) makes +Z point outward, so the
            // ring lies flat).
            const pin = d as CountryPin;
            const r = GLOBE_RADIUS * (1 + PILLAR_ALTITUDE);
            const { x, y, z } = latLngToXYZ(pin.lat, pin.lng, r);
            obj.position.set(x, y, z);
            obj.lookAt(0, 0, 0);
          }}
          arcsData={arcs}
          arcStartLat={(d: object) => (d as { startLat: number }).startLat}
          arcStartLng={(d: object) => (d as { startLng: number }).startLng}
          arcStartAltitude={OVERLAY_ALTITUDE}
          arcEndLat={(d: object) => (d as { endLat: number }).endLat}
          arcEndLng={(d: object) => (d as { endLng: number }).endLng}
          arcEndAltitude={OVERLAY_ALTITUDE}
          arcColor={() => ARC_COLOR}
          arcStroke={ARC_STROKE}
          arcDashLength={ARC_DASH_LENGTH}
          arcDashGap={ARC_DASH_GAP}
          arcDashAnimateTime={ARC_DASH_ANIM_MS}
          arcAltitude={(d: object) => (d as { altitude: number }).altitude}
          labelsData={labelsData}
          labelLat={(d: object) => (d as CountryPin).lat}
          labelLng={(d: object) => (d as CountryPin).lng}
          labelText={(d: object) =>
            // globe.gl uses the default Helvetiker typeface for labels,
            // which doesn't include Latin Extended glyphs (é, ô, ç, etc.).
            // Strip diacritics so names like "Saint Barthélemy" render
            // as "Saint Barthelemy" instead of "Saint Barth?lemy".
            (d as CountryPin).name
              .normalize("NFKD")
              .replace(/\p{Diacritic}/gu, "")
          }
          labelSize={labelSize}
          labelDotRadius={labelSize * LABEL_DOT_RADIUS_FRAC}
          labelAltitude={OVERLAY_ALTITUDE}
          labelColor={() => LABEL_COLOR}
          labelResolution={LABEL_RESOLUTION}
          labelIncludeDot={true}
          onLabelClick={onLabelClick}
          htmlElementsData={htmlElementsData}
          htmlLat={(d: object) => (d as CountryPin).lat}
          htmlLng={(d: object) => (d as CountryPin).lng}
          htmlAltitude={OVERLAY_ALTITUDE}
          htmlElement={(d: object) => {
            const pin = d as CountryPin;
            const el = document.createElement("img");
            el.src = flagUrl(pin.cca2);
            el.alt = pin.name;
            el.width = flagFontPx;
            el.height = flagFontPx;
            el.draggable = false;
            el.style.cursor = "pointer";
            el.style.userSelect = "none";
            el.style.transform = "translate(-50%, -50%)";
            el.style.filter = "drop-shadow(0 1px 1px rgba(0,0,0,0.5))";
            // globe.gl's HTML overlay container sets pointer-events:none
            // so canvas interactions still work through it; each child
            // has to opt back in to receive its own click.
            el.style.pointerEvents = "auto";
            el.style.zIndex = "1";
            el.title = pin.name;
            el.addEventListener("click", () => onLabelClick(pin));
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
          title="Bird's-eye view of swap route"
          className="flex h-9 w-9 items-center justify-center rounded-full border border-border bg-background/80 text-muted-fg shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent"
          aria-label="Bird's-eye view of swap route"
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
          disabled={altitude <= MIN_ALTITUDE + 0.01}
          aria-label="Zoom in"
        >
          <Plus size={16} />
        </button>
        <button
          type="button"
          onClick={zoomOut}
          title="Zoom out"
          className="flex h-9 w-9 items-center justify-center rounded-full border border-border bg-background/80 text-muted-fg shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
          disabled={altitude >= MAX_ALTITUDE - 0.01}
          aria-label="Zoom out"
        >
          <Minus size={16} />
        </button>
      </div>

      <CountryPickerDialog
        ctx={clickContext}
        top={pickerTop}
        onClose={closePicker}
        onPick={applyToSide}
      />
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
