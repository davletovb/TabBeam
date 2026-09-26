(() => {
  try {
    const value = localStorage.getItem("pervue.theme");
    if (value === "light" || value === "dark") {
      document.documentElement.setAttribute("data-theme", value);
    } else {
      document.documentElement.removeAttribute("data-theme");
    }
  } catch {
    document.documentElement.removeAttribute("data-theme");
  }
})();
