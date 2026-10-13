import { Layout } from "antd";

const { Sider, Header, Content } = Layout;

export default function App() {
  return (
    <Layout style={{ minHeight: "100vh" }}>
      <Sider>
        <div style={{ color: "#fff", padding: 16, fontWeight: 600 }}>
          Gist Auto Sync
        </div>
      </Sider>
      <Layout>
        <Header style={{ background: "#fff" }} />
        <Content style={{ padding: 24 }} />
      </Layout>
    </Layout>
  );
}
