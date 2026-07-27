import { Component, type ErrorInfo, type ReactNode } from "react";

type Props = {
  children: ReactNode;
};

type State = {
  error: Error | null;
};

/** Last-resort safety net: without this, any uncaught render error anywhere
 * in the tree unmounts the *entire* app, leaving a blank (near-black,
 * per our theme) page with no way back in short of relaunching. Catch it
 * here instead and offer a reload so one bad component can't brick the app. */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // eslint-disable-next-line no-console
    console.error("Unhandled error in render tree:", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div
          style={{
            minHeight: "100vh",
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            gap: 16,
            padding: 24,
            textAlign: "center",
            color: "#f5f5f4",
            background: "#141210",
          }}
        >
          <h2 style={{ margin: 0, fontSize: 18, fontWeight: 700 }}>
            Something went wrong
          </h2>
          <p style={{ margin: 0, maxWidth: 420, color: "#c4c0ba", fontSize: 13 }}>
            {this.state.error.message || "An unexpected error occurred."}
          </p>
          <button
            type="button"
            onClick={() => window.location.reload()}
            style={{
              border: "none",
              borderRadius: 10,
              padding: "10px 18px",
              background: "#2dd4bf",
              color: "#0a0a09",
              fontWeight: 700,
              fontSize: 13,
              cursor: "pointer",
            }}
          >
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
