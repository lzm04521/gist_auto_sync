import { HashRouter, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { App as AntApp, ConfigProvider, Layout, Menu, notification } from "antd";
import zhCN from "antd/locale/zh_CN";
import enUS from "antd/locale/en_US";
import {
  AppstoreOutlined,
  CloudSyncOutlined,
  FileTextOutlined,
  HistoryOutlined,
  InfoCircleOutlined,
  SettingOutlined,
  UserOutlined,
} from "@ant-design/icons";
import { useTranslation } from "react-i18next";
import Accounts from "./pages/Accounts";
import About from "./pages/About";
import { useStartupAutoCheck } from "./hooks/useUpdater";
import Dashboard from "./pages/Dashboard";
import History from "./pages/History";
import Logs from "./pages/Logs";
import Mappings from "./pages/Mappings";
import Settings from "./pages/Settings";

const { Sider, Header, Content } = Layout;

const menuItems = [
  { key: "/", icon: <AppstoreOutlined />, ns: "menu.dashboard" },
  { key: "/mappings", icon: <CloudSyncOutlined />, ns: "menu.mappings" },
  { key: "/history", icon: <HistoryOutlined />, ns: "menu.history" },
  { key: "/logs", icon: <FileTextOutlined />, ns: "menu.logs" },
  { key: "/accounts", icon: <UserOutlined />, ns: "menu.accounts" },
  { key: "/settings", icon: <SettingOutlined />, ns: "menu.settings" },
  { key: "/about", icon: <InfoCircleOutlined />, ns: "menu.about" },
];

function AppLayout() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();

  // 启动后延迟自动检查新版本；发现新版仅轻提示并引导到"关于"页（检查失败静默）
  useStartupAutoCheck((version) => {
    notification.info({
      message: t("about.foundNew", { version }),
      onClick: () => navigate("/about"),
    });
  });

  return (
    <Layout style={{ minHeight: "100vh" }}>
      <Sider theme="dark" width={200}>
        <div
          style={{
            color: "#fff",
            padding: 16,
            fontWeight: 600,
            fontSize: 16,
          }}
        >
          {t("app.title")}
        </div>
        <Menu
          theme="dark"
          mode="inline"
          selectedKeys={[location.pathname]}
          items={menuItems.map(({ key, icon, ns }) => ({
            key,
            icon,
            label: t(ns),
          }))}
          onClick={({ key }) => navigate(key)}
        />
      </Sider>
      <Layout>
        <Header style={{ background: "#fff", padding: "0 24px" }} />
        <Content style={{ padding: 24 }}>
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/mappings" element={<Mappings />} />
            <Route path="/history" element={<History />} />
            <Route path="/logs" element={<Logs />} />
            <Route path="/accounts" element={<Accounts />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="/about" element={<About />} />
          </Routes>
        </Content>
      </Layout>
    </Layout>
  );
}

export default function App() {
  const { i18n } = useTranslation();
  const antdLocale = i18n.language.startsWith("zh") ? zhCN : enUS;
  return (
    <ConfigProvider locale={antdLocale}>
      <AntApp>
        <HashRouter>
          <AppLayout />
        </HashRouter>
      </AntApp>
    </ConfigProvider>
  );
}
