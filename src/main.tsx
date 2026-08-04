import React from "react";
import ReactDOM from "react-dom/client";
import FileManagerApp from "./FileManagerApp";
import { installVersionLabelSync } from "./version-label";
import "./styles.css";
import "./advanced.css";
import "./file-manager.css";

installVersionLabelSync();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <FileManagerApp />
  </React.StrictMode>,
);
