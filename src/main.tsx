import React from "react";
import ReactDOM from "react-dom/client";
import "@ant-design/v5-patch-for-react-19";
import "antd/dist/reset.css";
import "./i18n";
import { initLanguage } from "./i18n";
import App from "./App";

// 启动语言从全局设置读取（spec：语言设置持久化）
void initLanguage();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
