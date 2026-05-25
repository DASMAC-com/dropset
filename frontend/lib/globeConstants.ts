// Three.js scene + react-globe.gl tuning constants for the globe panel.
// Grouped so a tweak to one camera limit doesn't require a grep for "0.05".

// ──────────────────────────── Colors ────────────────────────────

export const SELL_TINT = "#3b82f6"; // blue-500 — matches --accent
export const BUY_TINT = "#10b981"; // emerald-500
export const LAND_COVERED = "#64748b"; // slate-500 — supports a stable currency
export const LAND_UNCOVERED = "#1e293b"; // slate-800 — no supported currency
export const OCEAN_COLOR = 0x0b1726;
// Bright emerald-200 tying the cool palette together for the swap arc.
export const ARC_COLOR = "#a7f3d0";
export const ATMOSPHERE_COLOR = "#7dd3fc";
export const BACKGROUND_COLOR = "#020617";
export const LABEL_COLOR = "rgba(241, 245, 249, 0.95)";
export const RING_COLOR = "rgba(167, 243, 208, 0.45)";
export const POLY_SIDE_COLOR = "rgba(0,0,0,0.2)";
export const POLY_STROKE_COLOR = "rgba(255,255,255,0.18)";

// ────────────────────────── Geometry ─────────────────────────────

// three-globe's default sphere radius (the runtime arg isn't exposed in
// react-globe.gl's typings, but it's stable as long as we don't override).
export const GLOBE_RADIUS = 100;
// Height of the "same-country" pillar (a vertical cylinder over the shared
// anchor) and the spinning ring that sits on top of it.
export const PILLAR_ALTITUDE = 0.18;
export const ATMOSPHERE_ALTITUDE = 0.18;
// Overlay layers (rings/labels/arcs) — kept well above polygons.
export const OVERLAY_ALTITUDE = 0.018;
export const POLY_ALT_UNSUPPORTED = 0.008;
export const POLY_ALT_HIGHLIGHTED = 0.013;
export const POLY_ALT_DEFAULT = 0.011;

// Camera distance limits — let users dolly close enough that the Caribbean
// / Eurozone become separable, but not so far they lose context.
export const CAMERA_MIN_DISTANCE = 101;
export const CAMERA_MAX_DISTANCE = 600;

// Altitude clamps used by zoom/pan handlers.
export const MIN_ALTITUDE = 0.05;
export const MAX_ALTITUDE = 4.5;

// ───────────────────────── Camera defaults ──────────────────────

// Start centered roughly over the eastern US so auto-rotate reveals the
// Atlantic and then Europe — the canonical USD → EUR path.
export const DEFAULT_POV = { lat: 30, lng: -75, altitude: 1.5 };

// Negative rotates eastward.
export const AUTO_ROTATE_SPEED = -0.7;

// Below this altitude, country-name labels become visible.
export const LABEL_VISIBILITY_ALTITUDE = 2.6;

// Latitude tolerance (deg) used to declare the initial POV pan "settled",
// after which the canvas becomes visible. Without this, the user briefly
// sees the library's default POV before our snap commits.
export const POV_SETTLE_TOLERANCE_DEG = 0.5;
// Hard fallback for the settle check so a stuck poll doesn't leave the
// canvas invisible.
export const REVEAL_FALLBACK_MS = 1_500;

// Zoom step (multiplier per zoom button press / shortcut).
export const ZOOM_STEP = 1.3;
// Animation duration for zoom and pan transitions.
export const PAN_ANIMATION_MS = 250;
// Animation duration for focus-on-arc.
export const FOCUS_ARC_ANIMATION_MS = 800;
// Animation duration for reset-view.
export const RESET_VIEW_ANIMATION_MS = 800;

// ─────────────────── Animation tuning ───────────────────────────

// Per-frame Z rotation for the same-country ring (radians).
export const RING_SPIN_PER_FRAME = 0.03;
// Sweep angle (radians) for the partial torus so the spin reads as
// rotational rather than as a uniform disc.
export const RING_SWEEP_RAD = Math.PI * 1.5;
// Toggle interval (ms) for the polygon-cap flash when both sides share the
// exact same token (degenerate state — mirrors the disabled Swap button).
export const SAME_TOKEN_FLASH_MS = 500;

// Arc altitude formula constants (apex altitude scales with great-circle
// distance so near-antipodal pairs arch high enough to clear the globe
// surface instead of clipping through it).
export const ARC_BASE_ALTITUDE = 0.15;
export const ARC_DISTANCE_FACTOR = 0.4;
// Focus-on-arc altitude constants.
export const FOCUS_ARC_MIN_ALT = 1.4;
export const FOCUS_ARC_MAX_ALT = 3.0;
export const FOCUS_ARC_BASE_ALT = 1.2;
export const FOCUS_ARC_DIST_FACTOR = 0.9;

// Ring animation parameters.
export const RING_MAX_RADIUS = 1.6;
export const RING_PROPAGATION_SPEED = 0.7;
export const RING_REPEAT_PERIOD_MS = 2_200;

// Arc dash animation parameters.
export const ARC_DASH_LENGTH = 0.4;
export const ARC_DASH_GAP = 0.2;
export const ARC_DASH_ANIM_MS = 2_000;
export const ARC_STROKE = 0.8;

// Custom torus geometry for the spinning ring (in fractions of GLOBE_RADIUS).
export const RING_TORUS_RADIUS_FRAC = 0.04;
export const RING_TORUS_TUBE_FRAC = 0.006;
export const RING_TORUS_RAD_SEG = 8;
export const RING_TORUS_TUB_SEG = 48;

// ─────────────────── Sizing & layout ────────────────────────────

export const GLOBE_MIN_HEIGHT = 320;
export const GLOBE_MAX_HEIGHT = 480;
export const GLOBE_HEIGHT_RATIO = 0.85;
export const GLOBE_DEFAULT_WIDTH = 480;

// Pan step (deg) scaling formula: step = clamp(altitude * factor, min, max).
// Coarse pan when zoomed out, fine pan when zoomed in.
export const PAN_STEP_MIN_DEG = 2;
export const PAN_STEP_MAX_DEG = 20;
export const PAN_STEP_ALT_FACTOR = 10;
// Polar clamp so OrbitControls doesn't flip near the poles.
export const LAT_CLAMP_DEG = 85;

// Pinpoint clustering — used to drop the smaller pins inside any 4°-radius
// proximity cluster at the most zoomed-out label-visible altitude band.
export const FAR_ZOOM_PROXIMITY_DEG = 4;
// Below this altitude, every pin is shown; above it, only FAR_ZOOM_PINS.
export const FAR_ZOOM_ALTITUDE_THRESHOLD = 0.8;

// ─────────────────── Star layer parameters ──────────────────────

export const STAR_LAYERS: Array<{
  count: number;
  radius: number;
  size: number;
  opacity: number;
}> = [
  // Dense bed of faint pinpricks.
  { count: 2_500, radius: 700, size: 1.1, opacity: 0.55 },
  // Sparser brighter beacons that give the field a natural twinkle.
  { count: 280, radius: 700, size: 2.2, opacity: 1 },
];

// ─────────────────── Label sizing ───────────────────────────────

// Three-bucket label size — labels only rebuild when crossing a bucket
// boundary (a couple of times across a full zoom, not per frame).
export const LABEL_SIZE_CLOSE = 0.08;
export const LABEL_SIZE_MID = 0.32;
export const LABEL_SIZE_FAR = 1.4;
export const LABEL_BUCKET_CLOSE_MAX = 0.3;
export const LABEL_BUCKET_MID_MAX = 0.8;
// Dot radius is a fraction of the label size — kept proportional.
export const LABEL_DOT_RADIUS_FRAC = 0.36;
// Render resolution multiplier for the label material.
export const LABEL_RESOLUTION = 2;

// Mirrors LABEL_* buckets so flag emoji scale in step with the labels.
export const FLAG_FONT_PX_CLOSE = 18;
export const FLAG_FONT_PX_MID = 26;
export const FLAG_FONT_PX_FAR = 36;
