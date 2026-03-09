import { Routes, Route, Navigate } from "react-router-dom";
import {
    useState,
    useEffect,
    createContext,
    useContext,
    Component,
} from "react";
import type { ReactNode, ErrorInfo } from "react";
import Layout from "./components/layout/Layout";
import Dashboard from "./pages/Dashboard";
import AgentChat from "./pages/AgentChat";
import Tools from "./pages/Tools";
import Cron from "./pages/Cron";
import Integrations from "./pages/Integrations";
import Memory from "./pages/Memory";
import Config from "./pages/Config";
import Cost from "./pages/Cost";
import Logs from "./pages/Logs";
import Doctor from "./pages/Doctor";
import { AuthProvider, useAuth } from "./hooks/useAuth";
import { setLocale, type Locale } from "./lib/i18n";

// Locale context
interface LocaleContextType {
    locale: Locale;
    setAppLocale: (locale: Locale) => void;
}

export const LocaleContext = createContext<LocaleContextType>({
    locale: "tr" as Locale,
    setAppLocale: () => {},
});

export const useLocaleContext = () => useContext(LocaleContext);

// ---------------------------------------------------------------------------
// Error boundary — catches render crashes and shows a recoverable message
// instead of a black screen
// ---------------------------------------------------------------------------

interface ErrorBoundaryState {
    error: Error | null;
}

export class ErrorBoundary extends Component<
    { children: ReactNode },
    ErrorBoundaryState
> {
    constructor(props: { children: ReactNode }) {
        super(props);
        this.state = { error: null };
    }

    static getDerivedStateFromError(error: Error): ErrorBoundaryState {
        return { error };
    }

    componentDidCatch(error: Error, info: ErrorInfo) {
        console.error("[ZeroClaw] Render error:", error, info.componentStack);
    }

    render() {
        if (this.state.error) {
            return (
                <div className="p-6">
                    <div className="bg-gray-900 border border-red-700 rounded-xl p-6 w-full max-w-lg">
                        <h2 className="text-lg font-semibold text-red-400 mb-2">
                            Something went wrong
                        </h2>
                        <p className="text-gray-400 text-sm mb-4">
                            A render error occurred. Check the browser console
                            for details.
                        </p>
                        <pre className="text-xs text-red-300 bg-gray-800 rounded p-3 overflow-x-auto whitespace-pre-wrap break-all">
                            {this.state.error.message}
                        </pre>
                        <button
                            onClick={() => this.setState({ error: null })}
                            className="mt-6 px-4 py-2 bg-blue-600 hover:bg-blue-700 text-white text-sm font-medium rounded-lg transition-colors"
                        >
                            Try again
                        </button>
                    </div>
                </div>
            );
        }
        return this.props.children;
    }
}

// Pairing dialog component
function PairingDialog({
    onPair,
}: {
    onPair: (code: string) => Promise<void>;
}) {
    const [code, setCode] = useState("");
    const [error, setError] = useState("");
    const [loading, setLoading] = useState(false);

    const handleSubmit = async (e: React.FormEvent) => {
        e.preventDefault();
        setLoading(true);
        setError("");
        try {
            await onPair(code);
        } catch (err: unknown) {
            setError(err instanceof Error ? err.message : "Pairing failed");
        } finally {
            setLoading(false);
        }
    };

    return (
        <div className="min-h-screen bg-gray-950 flex items-center justify-center">
            <div className="bg-gray-900 rounded-xl p-8 w-full max-w-md border border-gray-800">
                <div className="text-center mb-6">
                    <h1 className="text-2xl font-bold text-white mb-2">
                        ZeroClaw
                    </h1>
                    <p className="text-gray-400">
                        Enter the pairing code from your terminal
                    </p>
                </div>
                <form onSubmit={handleSubmit}>
                    <input
                        type="text"
                        value={code}
                        onChange={(e) => setCode(e.target.value)}
                        placeholder="6-digit code"
                        className="w-full px-4 py-3 bg-gray-800 border border-gray-700 rounded-lg text-white text-center text-2xl tracking-widest focus:outline-none focus:border-blue-500 mb-4"
                        maxLength={6}
                        autoFocus
                    />
                    {error && (
                        <p className="text-red-400 text-sm mb-4 text-center">
                            {error}
                        </p>
                    )}
                    <button
                        type="submit"
                        disabled={loading || code.length < 6}
                        className="w-full py-3 bg-blue-600 hover:bg-blue-700 disabled:bg-gray-700 disabled:text-gray-500 text-white rounded-lg font-medium transition-colors"
                    >
                        {loading ? "Pairing..." : "Pair"}
                    </button>
                </form>
            </div>
        </div>
    );
}

function AppContent() {
    const { isAuthenticated, loading, pair, logout } = useAuth();
    const [locale, setLocaleState] = useState<Locale>("tr");

    const setAppLocale = (newLocale: Locale) => {
        setLocaleState(newLocale);
        setLocale(newLocale);
    };

    // Listen for 401 events to force logout
    useEffect(() => {
        const handler = () => {
            logout();
        };
        window.addEventListener("zeroclaw-unauthorized", handler);
        return () =>
            window.removeEventListener("zeroclaw-unauthorized", handler);
    }, [logout]);

    if (loading) {
        return (
            <div className="min-h-screen bg-gray-950 flex items-center justify-center">
                <p className="text-gray-400">Connecting...</p>
            </div>
        );
    }

    if (!isAuthenticated) {
        return <PairingDialog onPair={pair} />;
    }

    return (
        <LocaleContext.Provider value={{ locale, setAppLocale }}>
            <Routes>
                <Route element={<Layout />}>
                    <Route path="/" element={<Dashboard />} />
                    <Route path="/agent" element={<AgentChat />} />
                    <Route path="/tools" element={<Tools />} />
                    <Route path="/cron" element={<Cron />} />
                    <Route path="/integrations" element={<Integrations />} />
                    <Route path="/memory" element={<Memory />} />
                    <Route path="/config" element={<Config />} />
                    <Route path="/cost" element={<Cost />} />
                    <Route path="/logs" element={<Logs />} />
                    <Route path="/doctor" element={<Doctor />} />
                    <Route path="*" element={<Navigate to="/" replace />} />
                </Route>
            </Routes>
        </LocaleContext.Provider>
    );
}

export default function App() {
    return (
        <AuthProvider>
            <AppContent />
        </AuthProvider>
    );
}
