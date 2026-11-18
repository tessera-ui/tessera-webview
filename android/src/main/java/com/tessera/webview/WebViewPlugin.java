package com.tessera.webview;

import android.app.Activity;
import android.graphics.Bitmap;
import android.net.Uri;
import android.view.View;
import android.view.ViewGroup;
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
import android.widget.FrameLayout;

import androidx.webkit.WebViewCompat;
import androidx.webkit.WebViewFeature;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.ConcurrentHashMap;

public final class WebViewPlugin {
    private static final ConcurrentHashMap<String, Browser> BROWSERS = new ConcurrentHashMap<>();
    private static final ArrayBlockingQueue<JSONObject> EVENTS = new ArrayBlockingQueue<>(256);

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

    private static void event(String id, long generation, JSONObject event) {
        EVENTS.offer(object("id", Long.parseLong(id), "generation", generation,
            "payload", object("type", "event", "event", event)));
    }

    public static void create(Activity activity, String descriptor) {
        final JSONObject config;
        try { config = new JSONObject(descriptor); }
        catch (JSONException error) { throw new IllegalArgumentException(error); }
        final String id = config.optString("id");
        final long generation = config.optLong("generation");
        activity.runOnUiThread(() -> {
            try {
                Browser previous = BROWSERS.remove(id);
                if (previous != null) previous.destroy();
                Browser browser = new Browser(activity, id, generation, config);
                BROWSERS.put(id, browser);
                browser.emit(object("kind", "ready"));
                browser.load(config.getJSONObject("source"));
            } catch (Exception error) {
                Browser failed = BROWSERS.remove(id);
                if (failed != null) failed.destroy();
                event(id, generation, object("kind", "error", "error", object("Native", error.toString())));
            }
        });
    }

    public static void command(Activity activity, String id, String command) {
        activity.runOnUiThread(() -> {
            Browser browser = BROWSERS.get(id);
            if (browser == null) return;
            try {
                JSONObject action = new JSONObject(command);
                if (action.getLong("generation") != browser.generation) return;
                switch (action.getString("kind")) {
                    case "bounds": browser.bounds(action.getJSONArray("value")); break;
                    case "visible": browser.view.setVisibility(action.getBoolean("value") ? View.VISIBLE : View.INVISIBLE); break;
                    case "load": browser.load(action.getJSONObject("value")); break;
                    case "script": browser.view.evaluateJavascript(action.getString("value"), null); break;
                    case "evaluate":
                        long request = action.getLong("request");
                        browser.view.evaluateJavascript(action.getString("value"), value ->
                            EVENTS.offer(object("id", Long.parseLong(id), "generation", browser.generation,
                                "payload", object("type", "script", "request", request, "value", value == null ? "null" : value))));
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
                    case "destroy": BROWSERS.remove(id, browser); browser.destroy(); break;
                    default: throw new IllegalArgumentException("Unknown WebView command");
                }
            } catch (Exception error) {
                browser.emit(object("kind", "error", "error", object("Native", error.toString())));
            }
        });
    }

    public static String query(String id) {
        Browser browser = BROWSERS.get(id);
        return browser == null ? "{}" : browser.snapshot;
    }

    public static String poll(Activity activity) {
        JSONArray result = new JSONArray();
        for (int i = 0; i < 256; i++) {
            JSONObject event = EVENTS.poll();
            if (event == null) break;
            result.put(event);
        }
        return result.toString();
    }

    private static final class Browser {
        final String id;
        final long generation;
        final WebView view;
        final FrameLayout container;
        final JSONObject options;
        volatile String snapshot = "{}";
        float zoom = 1;

        Browser(Activity activity, String id, long generation, JSONObject config) throws JSONException {
            this.id = id;
            this.generation = generation;
            options = config.getJSONObject("options");
            container = new FrameLayout(activity);
            container.setClipChildren(true);
            view = new WebView(activity);
            view.setVisibility(View.INVISIBLE);
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
                    boolean blocked = !allows(request.getUrl().toString());
                    if (blocked && request.isForMainFrame()) emit(object("kind", "navigation_blocked", "url", request.getUrl().toString()));
                    return blocked;
                }
                @Override public android.webkit.WebResourceResponse shouldInterceptRequest(WebView ignored, WebResourceRequest request) {
                    // Android skips shouldOverrideUrlLoading for POST requests.
                    if (request.isForMainFrame() && !allows(request.getUrl().toString())) {
                        emit(object("kind", "navigation_blocked", "url", request.getUrl().toString()));
                        return new android.webkit.WebResourceResponse("text/plain", "UTF-8", 403, "Blocked", java.util.Collections.emptyMap(), new java.io.ByteArrayInputStream(new byte[0]));
                    }
                    return null;
                }
                @Override public void onPageStarted(WebView ignored, String url, Bitmap icon) {
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
                    view.destroy();
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
            container.addView(view, new FrameLayout.LayoutParams(-1, -1));
            ViewGroup content = activity.findViewById(android.R.id.content);
            content.addView(container, new ViewGroup.LayoutParams(-1, -1));
            bounds(config.getJSONArray("bounds"));
            container.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, oright, ob) -> updateSnapshot());
        }

        void updateSnapshot() {
            JSONArray bounds = new JSONArray(java.util.Arrays.asList(container.getX(), container.getY(), container.getWidth(), container.getHeight()));
            snapshot = object("url", view.getUrl(), "back", view.canGoBack(), "forward", view.canGoForward(), "bounds", bounds).toString();
        }
        void emit(JSONObject event) { WebViewPlugin.event(id, generation, event); }
        void bounds(JSONArray bounds) throws JSONException {
            // Tessera reports physical pixels; Android view coordinates are also
            // physical pixels, not density-independent layout dimensions.
            container.setX((float) bounds.getDouble(0));
            container.setY((float) bounds.getDouble(1));
            ViewGroup.LayoutParams params = container.getLayoutParams();
            params.width = Math.max(1, (int) Math.ceil(bounds.getDouble(2)));
            params.height = Math.max(1, (int) Math.ceil(bounds.getDouble(3)));
            container.setLayoutParams(params);
        }
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
            if (source.getString("kind").equals("url")) view.loadUrl(source.getString("value"));
            else view.loadDataWithBaseURL("about:blank", source.getString("value"), "text/html", "UTF-8", null);
        }
        void destroy() {
            if (container.getParent() instanceof ViewGroup) ((ViewGroup) container.getParent()).removeView(container);
            view.stopLoading();
            view.destroy();
        }
    }
}
