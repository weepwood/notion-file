import React from "react";
import ReactDOM from "react-dom/client";
import FileManagerApp from "./FileManagerApp";
import "./styles.css";
import "./advanced.css";
import "./file-manager.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <FileManagerApp />
  </React.StrictMode>,
);
