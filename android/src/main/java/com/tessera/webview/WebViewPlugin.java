package com.tessera.webview;

import android.app.Activity;
import android.graphics.Bitmap;
import android.net.Uri;
import android.view.View;
import android.webkit.CookieManager;
import android.webkit.GeolocationPermissions;
import android.webkit.PermissionRequest;
import android.webkit.SslErrorHandler;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebStorage;
import android.webkit.WebView;
import android.webkit.WebViewClient;

import androidx.webkit.WebViewCompat;
import androidx.webkit.WebViewFeature;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.ArrayBlockingQueue;

public final class WebViewPlugin {
    private WebViewPlugin() {}

    private static JSONObject object(Object... entries) {
        JSONObject result = new JSONObject();
        try {
            for (int i = 0; i < entries.length; i += 2) result.put((String) entries[i], entries[i + 1]);
        } catch (JSONException error) {
            throw new IllegalArgumentException(error);
        }
        return result;
    }

    public static boolean supportsPlatformViews(Activity activity) {
        return android.os.Build.VERSION.SDK_INT >= 30;
    }

    public static Factory newFactory(Activity activity, String descriptor) throws JSONException {
        return new Factory(activity, new JSONObject(descriptor));
    }

    public static Object platformFactory(Factory factory) throws ClassNotFoundException {
        // The generated application owns this interface; the AAR must not ship a copy.
        ClassLoader loader = factory.activity.getClassLoader();
        Class<?> contract = Class.forName("com.tessera.TesseraPlatformViewFactory", false, loader);
        return Proxy.newProxyInstance(loader, new Class<?>[]{contract}, factory);
    }

    public static void command(Factory factory, String command) {
        factory.activity.runOnUiThread(() -> {
            try {
                JSONObject action = new JSONObject(command);
                String kind = action.getString("kind");
                Browser browser = factory.browser;
                if (kind.equals("load")) {
                    factory.source = action.getJSONObject("value");
                    if (browser != null) browser.load(factory.source);
                    return;
                }
                if (browser == null) return;
                switch (kind) {
                    case "script": browser.view.evaluateJavascript(action.getString("value"), null); break;
                    case "evaluate":
                        long request = action.getLong("request");
                        browser.view.evaluateJavascript(action.getString("value"), value ->
                            factory.emit(object("type", "script", "request", request, "value", value == null ? "null" : value)));
                        break;
                    case "back": browser.view.goBack(); break;
                    case "forward": browser.view.goForward(); break;
                    case "reload": browser.view.reload(); break;
                    case "focus": browser.view.requestFocus(); break;
                    case "zoom":
                        float next = (float) action.getDouble("value");
                        browser.view.zoomBy(next / browser.zoom);
                        browser.zoom = next;
                        break;
                    case "clear_data":
                        browser.view.clearCache(true);
                        browser.view.clearHistory();
                        CookieManager.getInstance().removeAllCookies(null);
                        WebStorage.getInstance().deleteAllData();
                        break;
                    default: throw new IllegalArgumentException("Unknown WebView command");
                }
            } catch (Exception error) {
                factory.error(error);
            }
        });
    }

    public static String query(Factory factory) {
        return factory.snapshot;
    }

    public static String poll(Factory factory) {
        if (factory.events.isEmpty()) return "[]";
        JSONArray result = new JSONArray();
        for (int i = 0; i < 64; i++) {
            JSONObject event = factory.events.poll();
            if (event == null) break;
            result.put(event);
        }
        return result.toString();
    }

    public static final class Factory implements InvocationHandler {
        final Activity activity;
        final long id;
        final long generation;
        final JSONObject options;
        final ArrayBlockingQueue<JSONObject> events = new ArrayBlockingQueue<>(64);
        JSONObject source;
        Browser browser;
        volatile String snapshot = "{}";

        Factory(Activity activity, JSONObject config) throws JSONException {
            this.activity = activity;
            id = Long.parseLong(config.getString("id"));
            generation = config.getLong("generation");
            options = config.getJSONObject("options");
            source = config.getJSONObject("source");
        }

        void emit(JSONObject payload) {
            events.offer(object("id", id, "generation", generation, "payload", payload));
        }

        void event(JSONObject event) { emit(object("type", "event", "event", event)); }

        void error(Exception error) {
            event(object("kind", "error", "error", object("Native", error.toString())));
        }

        View create() {
            try {
                browser = new Browser(this);
                browser.configure();
                browser.load(source);
                browser.updateSnapshot();
                event(object("kind", "ready"));
                return browser.view;
            } catch (Exception error) {
                release();
                error(error);
                return new View(activity);
            }
        }

        void release() {
            if (browser != null) {
                browser.destroy();
                browser = null;
            }
            snapshot = "{}";
        }

        @Override public Object invoke(Object proxy, Method method, Object[] args) {
            switch (method.getName()) {
                case "create": return create();
                case "update":
                    if (browser != null) browser.updateSnapshot();
                    return null;
                case "reset": return null;
                case "release": release(); return null;
                case "hashCode": return System.identityHashCode(proxy);
                case "equals": return proxy == args[0];
                case "toString": return "TesseraWebViewFactory(" + id + ")";
                default: throw new UnsupportedOperationException(method.getName());
            }
        }
    }

    private static final class Browser {
        final Factory factory;
        final WebView view;
        final JSONObject options;
        float zoom = 1;
        volatile boolean inlineLoadPending;

        Browser(Factory factory) {
            this.factory = factory;
            options = factory.options;
            view = new WebView(factory.activity);
        }

        void configure() throws JSONException {
            WebSettings settings = view.getSettings();
            settings.setJavaScriptEnabled(options.optBoolean("javascript", true));
            settings.setDomStorageEnabled(true);
            settings.setAllowFileAccess(false);
            settings.setAllowContentAccess(false);
            settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
            settings.setMediaPlaybackRequiresUserGesture(true);
            settings.setSupportMultipleWindows(true);
            if (!options.isNull("user_agent")) settings.setUserAgentString(options.getString("user_agent"));
            JSONArray background = options.getJSONArray("background");
            view.setBackgroundColor(android.graphics.Color.argb(background.getInt(3), background.getInt(0), background.getInt(1), background.getInt(2)));
            view.setWebViewClient(new WebViewClient() {
                @Override public boolean shouldOverrideUrlLoading(WebView ignored, WebResourceRequest request) {
                    if (request.isForMainFrame() && isPendingInlineLoad(request.getUrl().toString())) return false;
                    boolean blocked = !allows(request.getUrl().toString());
                    if (blocked && request.isForMainFrame()) emit(object("kind", "navigation_blocked", "url", request.getUrl().toString()));
                    return blocked;
                }
                @Override public android.webkit.WebResourceResponse shouldInterceptRequest(WebView ignored, WebResourceRequest request) {
                    // Android skips shouldOverrideUrlLoading for POST requests.
                    if (request.isForMainFrame() && !allows(request.getUrl().toString())
                            && !isPendingInlineLoad(request.getUrl().toString())) {
                        emit(object("kind", "navigation_blocked", "url", request.getUrl().toString()));
                        return new android.webkit.WebResourceResponse("text/plain", "UTF-8", 403, "Blocked", java.util.Collections.emptyMap(), new java.io.ByteArrayInputStream(new byte[0]));
                    }
                    return null;
                }
                @Override public void onPageStarted(WebView ignored, String url, Bitmap icon) {
                    inlineLoadPending = false;
                    updateSnapshot();
                    emit(object("kind", "load_started", "url", url));
                    if (!WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) injectScripts();
                }
                @Override public void onPageFinished(WebView ignored, String url) {
                    updateSnapshot();
                    emit(object("kind", "load_finished", "url", url));
                }
                @Override public void onReceivedSslError(WebView ignored, SslErrorHandler handler, android.net.http.SslError error) { handler.cancel(); }
            });
            view.setWebChromeClient(new WebChromeClient() {
                @Override public void onReceivedTitle(WebView ignored, String title) { emit(object("kind", "title_changed", "title", title)); }
                @Override public void onPermissionRequest(PermissionRequest request) { request.deny(); }
                @Override public void onGeolocationPermissionsShowPrompt(String origin, GeolocationPermissions.Callback callback) { callback.invoke(origin, false, false); }
                @Override public boolean onCreateWindow(WebView ignored, boolean dialog, boolean gesture, android.os.Message result) {
                    // Android does not expose the requested URL before accepting
                    // the popup. Refuse creation instead of enabling a hidden view.
                    emit(object("kind", "new_window_requested", "url", ""));
                    return false;
                }
            });
            view.setDownloadListener((url, userAgent, disposition, mime, length) -> emit(object("kind", "download_requested", "url", url)));
            JSONObject ipc = options.getJSONObject("ipc");
            if (!ipc.getString("kind").equals("disabled")) {
                if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
                    throw new IllegalStateException("System WebView lacks origin-verified IPC; update Android System WebView");
                }
                Set<String> origins = new HashSet<>();
                if (ipc.getString("kind").equals("allow_all")) origins.add("*");
                else {
                    JSONArray allowed = ipc.getJSONArray("origins");
                    for (int i = 0; i < allowed.length(); i++) origins.add(allowed.getString(i));
                }
                WebViewCompat.addWebMessageListener(view, "ipc", origins, (webview, message, origin, mainFrame, reply) -> {
                    String body = message.getData();
                    if (mainFrame && body != null && body.getBytes(StandardCharsets.UTF_8).length <= 1024 * 1024)
                        emit(object("kind", "message", "url", origin.toString(), "body", body));
                });
            }
            if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
                JSONArray scripts = options.getJSONArray("initialization_scripts");
                for (int i = 0; i < scripts.length(); i++) WebViewCompat.addDocumentStartJavaScript(view, scripts.getString(i), java.util.Collections.singleton("*"));
            }
        }

        void updateSnapshot() {
            factory.snapshot = object("url", view.getUrl(), "back", view.canGoBack(), "forward", view.canGoForward()).toString();
        }
        void emit(JSONObject event) { factory.event(event); }
        boolean allows(String value) {
            if (value.equals("about:blank")) return true;
            Uri uri = Uri.parse(value);
            if (!("https".equals(uri.getScheme()) || "http".equals(uri.getScheme())) || uri.getHost() == null || uri.getUserInfo() != null) return false;
            JSONObject navigation = options.optJSONObject("navigation");
            if (navigation == null || navigation.optString("kind").equals("web")) return true;
            JSONArray origins = navigation.optJSONArray("origins");
            for (int i = 0; origins != null && i < origins.length(); i++) {
                Uri allowed = Uri.parse(origins.optString(i));
                int port = uri.getPort() == -1 ? (uri.getScheme().equals("https") ? 443 : 80) : uri.getPort();
                int expected = allowed.getPort() == -1 ? (allowed.getScheme().equals("https") ? 443 : 80) : allowed.getPort();
                if (uri.getScheme().equalsIgnoreCase(allowed.getScheme()) && uri.getHost().equalsIgnoreCase(allowed.getHost()) && port == expected) return true;
            }
            return false;
        }
        void injectScripts() {
            JSONArray scripts = options.optJSONArray("initialization_scripts");
            for (int i = 0; scripts != null && i < scripts.length(); i++) view.evaluateJavascript(scripts.optString(i), null);
        }
        void load(JSONObject source) throws JSONException {
            if (source.getString("kind").equals("url")) {
                inlineLoadPending = false;
                view.loadUrl(source.getString("value"));
            } else {
                inlineLoadPending = true;
                view.loadDataWithBaseURL("about:blank", source.getString("value"), "text/html", "UTF-8", null);
            }
        }
        boolean isPendingInlineLoad(String url) {
            return inlineLoadPending && url.startsWith("data:text/html");
        }
        void destroy() {
            view.stopLoading();
            view.destroy();
        }
    }
}
