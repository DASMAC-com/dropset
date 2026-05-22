// Pure spherical-geometry helpers used by the globe panel. No three.js or
// React imports so these are unit-testable independently of the renderer.

import type { CountryPin } from "./countries";
import {
  ARC_BASE_ALTITUDE,
  ARC_DISTANCE_FACTOR,
  FOCUS_ARC_BASE_ALT,
  FOCUS_ARC_DIST_FACTOR,
  FOCUS_ARC_MAX_ALT,
  FOCUS_ARC_MIN_ALT,
} from "./globeConstants";

const toRad = (d: number): number => (d * Math.PI) / 180;
const toDeg = (r: number): number => (r * 180) / Math.PI;

// Great-circle angular distance between two lat/lng points, in radians.
export const angularDistanceRad = (
  aLat: number,
  aLng: number,
  bLat: number,
  bLng: number,
): number => {
  const phi1 = toRad(aLat);
  const phi2 = toRad(bLat);
  const dPhi = toRad(bLat - aLat);
  const dLam = toRad(bLng - aLng);
  const h =
    Math.sin(dPhi / 2) ** 2 +
    Math.cos(phi1) * Math.cos(phi2) * Math.sin(dLam / 2) ** 2;
  return 2 * Math.atan2(Math.sqrt(h), Math.sqrt(1 - h));
};

// Same as angularDistanceRad but converts to degrees — used by the
// far-zoom clustering to thin out close-together pins.
export const angularDistanceDeg = (a: CountryPin, b: CountryPin): number =>
  toDeg(angularDistanceRad(a.lat, a.lng, b.lat, b.lng));

// Midpoint on the great circle between two lat/lng points, and the
// great-circle distance (radians). Used by focus-on-arc to point the
// camera at the route's midpoint with an altitude that keeps both
// endpoints in frame.
export const greatCircleMidpoint = (
  start: { lat: number; lng: number },
  end: { lat: number; lng: number },
): { lat: number; lng: number; angular: number } => {
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
  const angular = angularDistanceRad(start.lat, start.lng, end.lat, end.lng);
  return { lat: toDeg(midPhi), lng: toDeg(midLam), angular };
};

// Camera altitude that keeps both endpoints of a great-circle arc visible.
export const focusArcAltitude = (angularRad: number): number =>
  Math.max(
    FOCUS_ARC_MIN_ALT,
    Math.min(
      FOCUS_ARC_MAX_ALT,
      FOCUS_ARC_BASE_ALT + angularRad * FOCUS_ARC_DIST_FACTOR,
    ),
  );

// Apex altitude for the swap arc. Scales with great-circle distance so
// near-antipodal pairs arch high enough to clear the globe surface.
export const arcApexAltitude = (angularRad: number): number =>
  ARC_BASE_ALTITUDE + (angularRad / Math.PI) * ARC_DISTANCE_FACTOR;

// Cartesian position on a sphere of radius `r` for the given lat/lng (deg).
// Matches three-globe's coordinate convention so custom three.js objects
// can be placed at the same point as react-globe.gl's labels/pins.
export const latLngToXYZ = (
  lat: number,
  lng: number,
  r: number,
): { x: number; y: number; z: number } => {
  const phi = ((90 - lat) * Math.PI) / 180;
  const theta = ((lng + 90) * Math.PI) / 180;
  return {
    x: -r * Math.sin(phi) * Math.cos(theta),
    y: r * Math.cos(phi),
    z: r * Math.sin(phi) * Math.sin(theta),
  };
};
