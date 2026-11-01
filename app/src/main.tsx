import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/doto/400.css";
import "@fontsource/doto/700.css";
import "@fontsource/chakra-petch/400.css";
import "@fontsource/chakra-petch/500.css";
import "@fontsource/chakra-petch/600.css";
import "@fontsource/chakra-petch/700.css";
import "@fontsource/share-tech-mono/400.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/600.css";
import App from "./App";
import "./styles.css";

// Look like an app, not a web page: platform hook for the title-bar inset, and no browser
// context menu (Reload / Inspect) in release builds.
if (/Mac/i.test(navigator.platform)) document.documentElement.classList.add("mac");
if (import.meta.env.PROD) document.addEventListener("contextmenu", (e) => e.preventDefault());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
