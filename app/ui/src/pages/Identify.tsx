export function Identify({ label }: { label: string }) {
  return (
    <div className="flex h-full items-center justify-center rounded-3xl bg-[#111827] text-white">
      <div className="text-center">
        <div className="text-[64px] font-bold tracking-tight">{label}</div>
        <div className="mt-1 text-[18px] opacity-70">Nexpingdesk</div>
      </div>
    </div>
  );
}
