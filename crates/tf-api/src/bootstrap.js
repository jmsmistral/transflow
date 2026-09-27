// Launch grants and CSRF values stay in this closure; never persist credentials in web storage.
(async () => {
  const code = location.hash.slice(1);
  history.replaceState(null, "", location.pathname);
  const status = document.getElementById("status");
  try {
    const response = await fetch("/api/v1/sessions/exchange", {
      method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify({code}),
    });
    if (!response.ok) throw new Error("Launch refused");
    const session = await response.json();
    const verified = await fetch("/api/v1/sessions/verify", {
      method: "POST", headers: {"Content-Type": "application/json", "X-Transflow-CSRF": session.data.csrf}, body: "{}",
    });
    if (!verified.ok) throw new Error("Session refused");
    const capabilities = await fetch("/api/v1/read", {
      method: "POST", headers: {"Content-Type": "application/json", "X-Transflow-CSRF": session.data.csrf},
      body: JSON.stringify({path: "/api/v1/capabilities", query: {}}),
    });
    if (!capabilities.ok || (await capabilities.json()).data.api_version !== 1) throw new Error("Unsupported API");
    status.textContent = "Connected securely. The workspace interface is coming in a later task.";
  } catch {
    status.textContent = "This launch link is missing, expired or already used. Request a fresh link from the coordinator.";
  }
})();
