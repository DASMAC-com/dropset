"use client";

import { useEffect, useState } from "react";

// Bump when the terms text changes to force re-acceptance.
const TOS_VERSION = 1;
const STORAGE_KEY = `dropset.tos.v${TOS_VERSION}.acceptedAt`;

export function TermsOfUseGate() {
  const [open, setOpen] = useState(false);
  const [agreed, setAgreed] = useState(false);

  useEffect(() => {
    try {
      if (!window.localStorage.getItem(STORAGE_KEY)) setOpen(true);
    } catch {
      setOpen(true);
    }
  }, []);

  useEffect(() => {
    if (!open || !agreed) return;
    try {
      window.localStorage.setItem(STORAGE_KEY, new Date().toISOString());
    } catch {
      // Storage unavailable (private mode, quota); accept for this session anyway.
    }
    setOpen(false);
  }, [open, agreed]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-[60] bg-background/80 backdrop-blur-2xl"
      role="dialog"
      aria-modal="true"
      aria-labelledby="tos-title"
    >
      <div className="-translate-x-1/2 fixed top-6 left-1/2 z-[70] flex w-[min(420px,calc(100vw-2rem))] flex-col rounded-xl border border-border bg-background shadow-lg">
        <div className="border-border border-b px-5 py-3">
          <h2
            id="tos-title"
            className="font-semibold text-foreground text-base"
          >
            Terms of Use
          </h2>
        </div>
        <div className="scrollbar-always h-56 space-y-3 overflow-y-scroll px-5 py-3 text-muted-fg text-xs leading-relaxed">
          <p>
            This website-hosted user interface (this &ldquo;Interface&rdquo;) is
            an open source frontend software portal to the Dropset protocol, a
            decentralized and community-driven collection of blockchain-enabled
            smart contracts and tools (the &ldquo;Dropset Protocol&rdquo;). This
            Interface and the Dropset Protocol are made available by DASMAC,
            however all transactions conducted on the protocol are run by
            related permissionless smart contracts. As the Interface is
            open-sourced and the Dropset Protocol and its related smart
            contracts are accessible by any user, entity or third party, there
            are a number of third party web and mobile user-interfaces that
            allow for interaction with the Dropset Protocol.
          </p>
          <p>
            THIS INTERFACE AND THE DROPSET PROTOCOL ARE PROVIDED &ldquo;AS
            IS&rdquo;, AT YOUR OWN RISK, AND WITHOUT WARRANTIES OF ANY KIND.
            DASMAC does not provide, own, or control the Dropset Protocol or any
            transactions conducted on the protocol or via related smart
            contracts. By using or accessing this Interface or the Dropset
            Protocol and related smart contracts, you agree that no developer or
            entity involved in creating, deploying or maintaining this Interface
            or the Dropset Protocol will be liable for any claims or damages
            whatsoever associated with your use, inability to use, or your
            interaction with other users of, this Interface or the Dropset
            Protocol, including any direct, indirect, incidental, special,
            exemplary, punitive or consequential damages, or loss of profits,
            digital assets, tokens, or anything else of value.
          </p>
          <p>
            By using or accessing this Interface, the Dropset Protocol, or
            related smart contracts, you represent that you are not located in,
            incorporated or established in, or a citizen or resident of any
            jurisdiction that is subject to comprehensive sanctions or trade
            restrictions administered by the United States, including but not
            limited to Cuba, Iran, North Korea, Syria, the Crimea, Donetsk, and
            Luhansk regions, or any other jurisdiction in which accessing or
            using the Dropset Protocol is prohibited under United States law
            (the &ldquo;Prohibited Jurisdictions&rdquo;). You also represent
            that you are not subject to sanctions or otherwise designated on any
            list of prohibited or restricted parties or excluded or denied
            persons, including but not limited to the lists maintained by the
            United States&rsquo; Department of Treasury&rsquo;s Office of
            Foreign Assets Control.
          </p>
        </div>
        <div className="border-border border-t px-5 py-3">
          <label className="flex cursor-pointer items-center gap-2 text-foreground text-sm">
            <input
              type="checkbox"
              checked={agreed}
              onChange={(e) => setAgreed(e.target.checked)}
              className="h-4 w-4 accent-foreground"
            />
            I agree
          </label>
        </div>
      </div>
    </div>
  );
}
