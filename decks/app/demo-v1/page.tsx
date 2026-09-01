"use client";

import dynamic from "next/dynamic";
import { PresenterNotesExit } from "../PresenterNotesExit";

// Spectacle owns the DOM, keyboard nav, and (via styled-components) client
// styling — render it client-only so we skip styled-components' SSR registry
// dance and any hydration mismatch. The route is a full-screen deck; there's
// nothing to server-render for it anyway.
const DemoDeck = dynamic(() => import("./DemoDeck"), { ssr: false });

export default function DemoV1Page() {
  return (
    <>
      <DemoDeck />
      {/* Renders nothing outside presenter mode. Every deck route wants this,
          so a new deck folder should mount it the same way. */}
      <PresenterNotesExit />
    </>
  );
}
