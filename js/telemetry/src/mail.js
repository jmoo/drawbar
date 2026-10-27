// A report as the email the collector sends. Only the id and kind, which `report()`
// has already matched against fixed patterns, reach a header; everything the sender
// wrote stays in the plain-text body.

// A contact that may become the email's Reply-To.
const ADDRESS = /^[^\s@<>()[\]\\,;:"]+@[^\s@<>()[\]\\,;:"]+\.[^\s@<>()[\]\\,;:"]+$/;

export function message(sent) {
  const reply = ADDRESS.test(sent.contact) ? { replyTo: sent.contact } : {};
  return {
    from: { email: "noreply@johndm.dev", name: "drawbar" },
    to: "reports@drawbar.app",
    subject: `drawbar ${sent.kind} ${sent.id}`,
    text: body(sent),
    // A retry of the same report threads under the first.
    headers: { References: `<${sent.id}@drawbar.app>` },
    ...reply,
  };
}

function body(sent) {
  const fields = ["contact", "version", "model", "firmware"]
    .map((name) => `${name}: ${sent[name] || "none"}`)
    .join("\n");
  const attached = [
    ["recent errors", sent.faults],
    ["build", sent.build],
    ["log", sent.log],
  ]
    .filter(([, value]) => value)
    .map(([name, value]) => `${name}:\n${value}`);
  return [fields, sent.text, ...attached].join("\n\n");
}
