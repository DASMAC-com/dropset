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
        {/* Nav links are hidden on phones — mobile is a swap-only experience
            (the currencies/vaults pages redirect to /swap below `md`, see
            MobileSwapRedirect), so there's nothing to navigate to. */}
        <nav className="hidden items-center gap-2 md:flex">
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
            <span className="hidden h-9 items-center rounded-md border border-amber-500/50 bg-amber-500/15 px-2 font-semibold text-[10px] text-amber-400 uppercase tracking-wide sm:inline-flex">
              Preview · mock data
            </span>
          )}
        </nav>
        <button
          type="button"
          onClick={() => emit("toggleHelp")}
          aria-label="Show keyboard shortcuts"
          title="Keyboard shortcuts (?)"
          className="ml-auto hidden h-9 w-9 items-center justify-center rounded-md text-foreground hover:bg-muted sm:inline-flex"
        >
          <Keyboard size={18} />
        </button>
        <a
          href="https://x.com/__Dropset__"
          target="_blank"
          rel="noopener noreferrer"
          className="ml-auto inline-flex h-9 w-9 items-center justify-center rounded-md text-foreground hover:bg-muted sm:ml-0"
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
        <WalletButton />
      </div>
    </header>
  );
}
