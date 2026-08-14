document.getElementById("ping").addEventListener("click", () => {
    document.getElementById("output").textContent =
        `✔ JS 正常工作 — ${new Date().toLocaleTimeString()}`;
});
