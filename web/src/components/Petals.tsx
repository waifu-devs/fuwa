const PETALS = [
  { left: "8%", delay: "0s", duration: "14s" },
  { left: "22%", delay: "5s", duration: "18s" },
  { left: "41%", delay: "2s", duration: "16s" },
  { left: "63%", delay: "8s", duration: "20s" },
  { left: "79%", delay: "3s", duration: "15s" },
  { left: "92%", delay: "10s", duration: "19s" },
];

export function Petals() {
  return (
    <div aria-hidden className="petals">
      {PETALS.map((p) => (
        <span key={p.left} style={{ left: p.left, animationDelay: p.delay, animationDuration: p.duration }} />
      ))}
    </div>
  );
}
