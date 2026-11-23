import { StrictMode, lazy, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./style.css";

const root = createRoot(document.getElementById("root")!);
const params = new URLSearchParams(window.location.search);
const Preview = import.meta.env.DEV && params.has("preview")
  ? lazy(() => import("./Preview"))
  : null;

root.render(
  <StrictMode>
    {Preview ? <Suspense><Preview /></Suspense> : <App />}
  </StrictMode>
);
