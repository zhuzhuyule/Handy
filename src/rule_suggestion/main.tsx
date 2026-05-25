import "@radix-ui/themes/styles.css";
import React from "react";
import ReactDOM from "react-dom/client";
import "../App.css";
import { ErrorBoundary } from "../components/ErrorBoundary";
import { RadixThemeProvider } from "../components/theme/RadixThemeProvider";
import "../i18n";
import { RuleSuggestionWindow } from "./RuleSuggestionWindow";

const params = new URLSearchParams(window.location.search);

const payload = {
  appName: params.get("app") ?? "",
  title: params.get("title") ?? "",
  promptName: params.get("prompt") ?? "",
  promptId: params.get("pid") ?? "",
  count: Number(params.get("count") ?? 0),
  threshold: Number(params.get("threshold") ?? 0),
};

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <RadixThemeProvider>
        <RuleSuggestionWindow payload={payload} />
      </RadixThemeProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);
