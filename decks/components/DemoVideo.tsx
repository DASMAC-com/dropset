"use client";

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { colors, deckTheme } from "@/theme/tokens";

/**
 * A demo beat's recorded video: a badge that sits on the slide and opens the
 * recording over the whole window when the presenter clicks (or hits Enter on)
 * it. The badge also names the network the demo was recorded on, since a
 * mainnet run and a bootstrapped localnet book are different claims.
 *
 * Why an overlay rather than an embed on the slide: a slide-sized player is
 * too small to read from a room, and Spectacle scales each slide to fit the
 * viewport, so anything laid out inside it inherits that scale. The overlay is
 * rendered through a portal to `document.body` instead — a Spectacle slide sits
 * under a CSS transform, and `position: fixed` inside a transformed ancestor is
 * contained by that ancestor rather than the viewport, so an in-tree overlay
 * would be trapped in the slide box it was trying to escape.
 */
export type DemoVideoProps = {
  /** The network the recording was made on, shown on the badge. */
  network: string;
  /** YouTube id. Shorts ids work as-is: the /shorts/<id> path is a viewer, and
   *  the id embeds through the ordinary /embed/<id> route. */
  videoId: string;
  /** Shorts are shot 9:16; a landscape screen capture is 16:9. */
  portrait?: boolean;
};

export const DemoVideo = ({
  network,
  videoId,
  portrait = false,
}: DemoVideoProps) => {
  const [open, setOpen] = useState(false);
  // The portal target only exists in the browser, and this deck is rendered
  // client-only, so wait for the first effect before reaching for it.
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);

  useEffect(() => {
    if (!open) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setOpen(false);
        return;
      }
      // Spectacle's arrow-key navigation is bound on the document, so without
      // this the deck advances underneath an open video and the presenter
      // closes it to find themselves two slides along.
      if (event.key.startsWith("Arrow") || event.key === " ") {
        event.stopPropagation();
      }
    };
    // Capture phase, so this runs before the deck's own handler.
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open]);

  // `youtube-nocookie` so a public deck doesn't set tracking cookies for every
  // viewer; it serves the same player and the same ids.
  const src = `https://www.youtube-nocookie.com/embed/${videoId}?autoplay=1&rel=0&playsinline=1&modestbranding=1`;
  const label = `demo video · ${network}`;

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        aria-label={`Play the ${network} demo video`}
        style={{
          background: "none",
          border: `1px solid ${colors.accent}`,
          borderRadius: "6px",
          color: colors.accent,
          cursor: "pointer",
          fontFamily: deckTheme.fonts.monospace,
          fontSize: "19px",
          lineHeight: 1.2,
          padding: "8px 15px",
        }}
      >
        ▶ {label}
      </button>

      {mounted && open
        ? createPortal(
            <div
              role="presentation"
              onClick={() => setOpen(false)}
              style={{
                alignItems: "center",
                backgroundColor: "rgba(0, 0, 0, 0.94)",
                display: "flex",
                inset: 0,
                justifyContent: "center",
                position: "fixed",
                zIndex: 2000,
              }}
            >
              <div
                role="presentation"
                onClick={(event) => event.stopPropagation()}
                style={{
                  aspectRatio: portrait ? "9 / 16" : "16 / 9",
                  backgroundColor: colors.background,
                  border: `1px solid ${colors.border}`,
                  borderRadius: "12px",
                  height: portrait ? "86vh" : "auto",
                  maxWidth: "94vw",
                  overflow: "hidden",
                  width: portrait ? "auto" : "min(92vw, 1280px)",
                }}
              >
                <iframe
                  src={src}
                  title={`Dropset ${label}`}
                  // Only what playback needs. YouTube's stock embed string also
                  // delegates accelerometer, gyroscope, clipboard-write and
                  // web-share, none of which a recording uses.
                  allow="autoplay; encrypted-media; picture-in-picture"
                  referrerPolicy="strict-origin-when-cross-origin"
                  allowFullScreen
                  style={{
                    border: 0,
                    display: "block",
                    height: "100%",
                    width: "100%",
                  }}
                />
              </div>
              <button
                type="button"
                onClick={() => setOpen(false)}
                style={{
                  background: "none",
                  border: `1px solid ${colors.border}`,
                  borderRadius: "6px",
                  color: colors.mutedFg,
                  cursor: "pointer",
                  fontFamily: deckTheme.fonts.monospace,
                  fontSize: "16px",
                  padding: "6px 12px",
                  position: "fixed",
                  right: "24px",
                  top: "24px",
                }}
              >
                esc
              </button>
            </div>,
            document.body,
          )
        : null}
    </>
  );
};
