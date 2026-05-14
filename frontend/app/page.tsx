import { GlobePanel } from "@/components/GlobePanel";
import { KeyboardShortcuts } from "@/components/KeyboardShortcuts";
import { SwapPanel } from "@/components/SwapPanel";

export default function Home() {
  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-6 py-10">
      <KeyboardShortcuts />
      <SwapPanel />
      <GlobePanel />
    </div>
  );
}
