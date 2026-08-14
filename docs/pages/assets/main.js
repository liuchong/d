// d — project pages interactions

// Copy-to-clipboard for install commands.
document.querySelectorAll("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
        const target = document.querySelector(btn.dataset.copy);
        const text = target ? target.textContent.trim() : "";
        try {
            await navigator.clipboard.writeText(text);
            btn.classList.add("copied");
            btn.textContent = "copied ✓";
        } catch {
            btn.textContent = "select & copy manually";
        }
        setTimeout(() => {
            btn.classList.remove("copied");
            btn.textContent = "copy";
        }, 1600);
    });
});

// Subtle blinking cursor on the hero title.
document.querySelectorAll(".blink").forEach((el) => {
    setInterval(() => {
        el.style.opacity = el.style.opacity === "0" ? "1" : "0";
    }, 600);
});
