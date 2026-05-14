import { GlobePanel } from "@/components/GlobePanel";
import { KeyboardShortcuts } from "@/components/KeyboardShortcuts";
import { ShortcutsHelp } from "@/components/ShortcutsHelp";
import { SwapPanel } from "@/components/SwapPanel";

export default function Home() {
  return (
    <div className="mx-auto flex max-w-[575px] flex-col gap-3 px-6 pt-3 pb-10">
      <KeyboardShortcuts />
      <ShortcutsHelp />
      <SwapPanel />
      <GlobePanel />
    </div>
  );
}
