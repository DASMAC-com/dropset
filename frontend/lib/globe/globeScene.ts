// Three.js scene helpers used by the globe panel. Kept independent of
// react-globe.gl so the construction stays unit-testable and can be reused
// if we ever swap the renderer.

import {
  BufferAttribute,
  BufferGeometry,
  Points,
  PointsMaterial,
  type Scene,
} from "three";
import { STAR_LAYERS } from "./globeConstants";

// Procedurally-generated star layer — a Points object placed at a fixed
// world position, so as the OrbitControls camera moves the stars appear
// to drift across the sky, anchored to the scene.
function makeStarLayer({
  count,
  radius,
  size,
  opacity,
}: (typeof STAR_LAYERS)[number]): Points {
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

// Add every configured star layer to the scene and return a disposer that
// removes and frees them. Use in an effect alongside react-globe.gl mount.
export function installStarLayers(scene: Scene): () => void {
  const layers = STAR_LAYERS.map(makeStarLayer);
  for (const layer of layers) scene.add(layer);
  return () => {
    for (const layer of layers) {
      scene.remove(layer);
      layer.geometry.dispose();
      (layer.material as PointsMaterial).dispose();
    }
  };
}
