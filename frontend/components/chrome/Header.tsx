"use client";

import Image from "next/image";
import Link from "next/link";
import { usePathname } from "next/navigation";
import type { MouseEvent } from "react";
import { Keyboard } from "@/components/icons";
import { WalletButton } from "@/components/wallet/WalletButton";
import { emit } from "@/lib/events";

const navClass = (active: boolean) =>
  active
    ? "inline-flex h-9 items-center rounded-md border border-muted-fg/40 bg-foreground/[0.07] px-3 font-medium text-foreground text-sm no-underline"
    : "inline-flex h-9 items-center rounded-md px-3 font-medium text-muted-fg text-sm no-underline hover:bg-muted hover:text-foreground";

// After a mouse click the link keeps DOM focus, so its focus outline lingers
// on the pill even after we've navigated away. Drop focus on click; keyboard
// users still get the focus ring (they don't trigger this path on Enter).
const blurOnClick = (e: MouseEvent<HTMLAnchorElement>) =>
  e.currentTarget.blur();

// No `display` utility here — each slot supplies its own (`inline-flex`,
// `hidden sm:inline-flex`, etc.). Baking `inline-flex` in would collide with
// a caller's `hidden`: both set `display`, and the unprefixed `inline-flex`
// wins by stylesheet order, so the element would never hide.
const iconButtonClass =
  "h-9 w-9 items-center justify-center rounded-md text-foreground hover:bg-muted";

// The link to Dropset's X account. Rendered in two slots (beside the logo on
// phones, in the right-hand cluster on desktop) with only one visible per
// breakpoint, so caller passes the responsive visibility class.
function XLink({ className }: { className: string }) {
  return (
    <a
      href="https://x.com/__Dropset__"
      target="_blank"
      rel="noopener noreferrer"
      className={`${iconButtonClass} ${className}`}
    >
      <span className="sr-only">Dropset on X</span>
      <svg
        viewBox="0 0 24 24"
        className="h-4 w-4"
        fill="currentColor"
        aria-hidden="true"
      >
        <title>X</title>
        <path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" />
      </svg>
    </a>
  );
}

export function Header() {
  const pathname = usePathname();
  return (
    <header className="sticky top-0 z-50 border-border border-b bg-background">
      <div className="mx-auto flex h-14 max-w-6xl items-center gap-2 px-3 sm:px-6">
        <Link
          href="/swap"
          aria-label="Dropset"
          className="mr-2 flex shrink-0 items-center no-underline sm:mr-4"
        >
          <Image
            src="/favicon.png"
            alt=""
            width={36}
            height={36}
            priority
            suppressHydrationWarning
          />
        </Link>
        {/* On narrow widths the nav is hidden, so the X link sits beside the
            logo to fill the empty left side. Wider keeps it in the right
            cluster. */}
        <XLink className="inline-flex sm:hidden" />
        {/* Nav links are hidden below `sm` — that's roughly phone width; real
            mobile devices are redirected to /swap anyway (MobileSwapRedirect).
            A laptop at half width (≥640px) still shows the full nav. */}
        <nav className="hidden items-center gap-2 sm:flex">
          <Link
            href="/swap"
            aria-current={pathname === "/swap" ? "page" : undefined}
            className={navClass(pathname === "/swap")}
            onClick={blurOnClick}
          >
            Swap
          </Link>
          <Link
            href="/currencies"
            aria-current={pathname === "/currencies" ? "page" : undefined}
            className={navClass(pathname === "/currencies")}
            onClick={blurOnClick}
          >
            Currencies
          </Link>
          <Link
            href="/vaults"
            aria-current={pathname === "/vaults" ? "page" : undefined}
            className={navClass(pathname === "/vaults")}
            onClick={blurOnClick}
          >
            Vaults
          </Link>
          {pathname === "/vaults" && (
            // shrink-0 + whitespace-nowrap so the pill never gets squeezed
            // into a multi-line wrap that overflows its fixed h-9 height. In
            // the tight sm–md band only "Preview" shows; the "· mock data"
            // suffix appears from md up, where the header has room for it.
            <span className="inline-flex h-9 shrink-0 items-center whitespace-nowrap rounded-md border border-amber-500/50 bg-amber-500/15 px-2 font-semibold text-[10px] text-amber-400 uppercase tracking-wide">
              Preview
              <span className="hidden md:inline">&nbsp;· mock data</span>
            </span>
          )}
        </nav>
        {/* Right-hand controls. ml-auto on the wrapper pushes the whole cluster
            to the right edge on every screen; below `sm` it's just the wallet
            button (keyboard shortcuts and the X link show from `sm` up). */}
        <div className="ml-auto flex items-center gap-2">
          <button
            type="button"
            onClick={() => emit("toggleHelp")}
            aria-label="Show keyboard shortcuts"
            title="Keyboard shortcuts (?)"
            className={`hidden sm:inline-flex ${iconButtonClass}`}
          >
            <Keyboard size={18} />
          </button>
          <XLink className="hidden sm:inline-flex" />
          <WalletButton />
        </div>
      </div>
    </header>
  );
}
