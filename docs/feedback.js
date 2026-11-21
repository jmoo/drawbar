// The guide's Feedback page. Sends what the reader wrote to drawbar's collector as a
// report of kind `feedback`, holding exactly the fields the collector accepts.

"use strict";

(() => {
  const form = document.getElementById("feedback");
  if (!form) {
    return;
  }
  // The collector answers no other origin.
  const ORIGIN = "https://drawbar.app";
  const COLLECTOR = "https://t.drawbar.app/report";
  // The app's report numbers use the same alphabet, which has no look-alikes.
  const ALPHABET = "23456789abcdefghjkmnpqrstuvwxyz";
  const said = form.querySelector("[role=status]");
  const send = form.querySelector("button");

  form.hidden = false;
  if (location.origin !== ORIGIN) {
    send.disabled = true;
    said.textContent = "This form sends from drawbar.app only.";
    return;
  }

  const number = () =>
    Array.from(
      crypto.getRandomValues(new Uint8Array(10)),
      (byte) => ALPHABET[byte % ALPHABET.length],
    ).join("");

  const refused = (status) =>
    status === 429
      ? "drawbar has had too many reports just now. Try again in a minute."
      : `The feedback was not accepted (error ${status}).`;

  // A message keeps its number until it arrives, so a second try is the same report.
  let id = null;

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const text = form.elements.text.value.trim();
    if (!text) {
      return;
    }
    id ??= number();
    const body = JSON.stringify({
      id,
      kind: "feedback",
      text,
      contact: form.elements.contact.value.trim(),
      version: "guide",
      model: "",
      firmware: "",
      faults: "",
      build: "",
      log: "",
    });
    send.disabled = true;
    said.textContent = "Sending…";
    let answer = null;
    try {
      // `text/plain` is a simple request, so the browser sends no preflight.
      answer = await fetch(COLLECTOR, {
        method: "POST",
        headers: { "content-type": "text/plain" },
        body,
      });
    } catch {
      // Left null: offline, or the collector was out of reach.
    }
    send.disabled = false;
    if (answer === null) {
      said.textContent = "drawbar could not reach its server. Try again in a minute.";
      return;
    }
    if (!answer.ok) {
      said.textContent = refused(answer.status);
      return;
    }
    said.textContent = `Thank you. Your feedback was sent. Its number: ${id}. To have it deleted, write to contact@drawbar.app with that number.`;
    form.reset();
    id = null;
  });
})();
