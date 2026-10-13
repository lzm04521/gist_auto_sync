import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import App from "./App";

describe("App scaffold", () => {
  it("renders antd layout shell with app name", () => {
    render(<App />);
    expect(screen.getByText("Gist Auto Sync")).toBeInTheDocument();
  });
});
