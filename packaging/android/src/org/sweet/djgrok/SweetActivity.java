package org.sweet.djgrok;

import android.app.NativeActivity;
import android.os.Build;
import android.os.Bundle;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.SurfaceView;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.view.Window;
import android.view.WindowManager;
import android.content.Context;
import android.graphics.Color;
import android.graphics.Rect;
import android.text.InputType;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.PopupWindow;
import android.text.Editable;
import android.text.TextWatcher;
import android.util.TypedValue;
import dev.ffmpegkit_maintained.ytdlp.YtDlp;
import dev.ffmpegkit_maintained.ytdlp.YtDlpException;

public final class SweetActivity extends NativeActivity {
    // NativeActivity loads the same library for android_main, but that does
    // not register it with this Activity's class loader for Java native calls.
    static {
        System.loadLibrary("sweet");
    }

    private EditText sweetInput;
    private PopupWindow sweetInputPopup;
    private boolean syncingInput;
    private int lastViewportHeight = -1;
    private int lastInputType;
    private int mobileSafeTopInsetPx;
    private int mobileSafeBottomInsetPx;
    private View mobileSurface;

    public String sweetYtdlpExtractInfo(String source, boolean flat) {
        try {
            return YtDlp.extractInfoJson(this, source, flat, 20);
        } catch (YtDlpException error) {
            throw new RuntimeException(error.getMessage(), error);
        }
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        sweetInput = new EditText(this);
        sweetInput.setSingleLine(true);
        sweetInput.setTextColor(Color.TRANSPARENT);
        sweetInput.setBackgroundColor(Color.TRANSPARENT);
        sweetInput.setAlpha(0f);
        sweetInput.setIncludeFontPadding(false);
        lastInputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
        sweetInput.setInputType(lastInputType);
        sweetInput.setPadding(0, 0, 0, 0);
        sweetInput.setGravity(Gravity.CENTER_VERTICAL | Gravity.START);
        sweetInput.setImeOptions(EditorInfo.IME_ACTION_DONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI);
        sweetInput.setFocusable(true);
        sweetInput.setFocusableInTouchMode(true);
        sweetInput.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        sweetInputPopup = new PopupWindow(sweetInput, 1, 1, true);
        sweetInputPopup.setBackgroundDrawable(new android.graphics.drawable.ColorDrawable(Color.TRANSPARENT));
        sweetInputPopup.setInputMethodMode(PopupWindow.INPUT_METHOD_NEEDED);
        sweetInputPopup.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        sweetInputPopup.setClippingEnabled(false);
        View root = getWindow().getDecorView().getRootView();
        root.getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            Rect visible = new Rect();
            root.getWindowVisibleDisplayFrame(visible);
            int[] rootLocation = new int[2];
            root.getLocationOnScreen(rootLocation);
            int height = Math.max(0, visible.bottom - rootLocation[1]
                    - mobileSafeTopInsetPx - mobileSafeBottomInsetPx);
            resizeNativeSurface(root, height);
            if (height > 0 && height != lastViewportHeight) {
                lastViewportHeight = height;
                nativeViewportChanged(height);
            }
        });
        sweetInput.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
            @Override public void onTextChanged(CharSequence s, int start, int before, int count) {}
            @Override public void afterTextChanged(Editable value) {
                if (!syncingInput) {
                    nativeTextChanged(value.toString());
                }
            }
        });
    }

    public void sweetSetMobileSafeArea(boolean reserveTop, boolean reserveBottom) {
        runOnUiThread(() -> {
            Window window = getWindow();
            window.setStatusBarColor(Color.TRANSPARENT);
            window.setNavigationBarColor(Color.TRANSPARENT);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                window.setStatusBarContrastEnforced(false);
                window.setNavigationBarContrastEnforced(false);
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) window.setDecorFitsSystemWindows(false);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                WindowManager.LayoutParams attributes = window.getAttributes();
                attributes.layoutInDisplayCutoutMode =
                        WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
                window.setAttributes(attributes);
            }
            View decor = window.getDecorView();
            decor.setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);

            View content = findViewById(android.R.id.content);
            content.setOnApplyWindowInsetsListener((view, insets) -> {
                int topInset = insets.getSystemWindowInsetTop();
                int bottomInset = insets.getSystemWindowInsetBottom();
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P
                        && insets.getDisplayCutout() != null) {
                    topInset = Math.max(topInset, insets.getDisplayCutout().getSafeInsetTop());
                }
                mobileSafeTopInsetPx = reserveTop ? topInset : 0;
                mobileSafeBottomInsetPx = reserveBottom ? bottomInset : 0;
                resizeNativeSurface(getWindow().getDecorView().getRootView(),
                        Math.max(0, view.getRootView().getHeight()
                                - mobileSafeTopInsetPx - mobileSafeBottomInsetPx));
                return insets;
            });
            content.requestApplyInsets();
        });
    }

    private void resizeNativeSurface(View root, int height) {
        if (mobileSurface == null || mobileSurface.getParent() == null) {
            mobileSurface = findSurfaceView(root);
        }
        if (mobileSurface == null) return;
        ViewGroup.LayoutParams params = mobileSurface.getLayoutParams();
        if (params != null && params.height != height) {
            params.height = height;
            mobileSurface.setLayoutParams(params);
        }
        float top = mobileSafeTopInsetPx;
        if (mobileSurface.getTranslationY() != top) mobileSurface.setTranslationY(top);
    }

    private View findSurfaceView(View view) {
        if (view instanceof SurfaceView) return view;
        if (!(view instanceof ViewGroup)) return null;
        ViewGroup group = (ViewGroup) view;
        for (int index = 0; index < group.getChildCount(); index++) {
            View found = findSurfaceView(group.getChildAt(index));
            if (found != null) return found;
        }
        return null;
    }

    public void sweetSetImeAllowed(boolean allowed) {
        runOnUiThread(() -> {
            if (sweetInput == null) return;
            InputMethodManager manager = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
            if (allowed) {
                sweetInput.requestFocus();
                manager.showSoftInput(sweetInput, InputMethodManager.SHOW_IMPLICIT);
            } else {
                manager.hideSoftInputFromWindow(sweetInput.getWindowToken(), 0);
                sweetInput.clearFocus();
            }
        });
    }

    public void sweetSetInputField(String value, String hint, float x, float y, float width, float height,
                                   float fontSize, int color, boolean password, boolean multiline) {
        runOnUiThread(() -> {
            if (sweetInput == null) return;
            sweetInput.setSingleLine(!multiline);
            sweetInput.setGravity(multiline
                    ? Gravity.TOP | Gravity.START
                    : Gravity.CENTER_VERTICAL | Gravity.START);
            if (!sweetInput.getText().toString().equals(value)) {
                syncingInput = true;
                sweetInput.setText(value);
                sweetInput.setSelection(value.length());
                syncingInput = false;
            }
            int inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
            if (password) inputType |= InputType.TYPE_TEXT_VARIATION_PASSWORD;
            if (multiline) inputType |= InputType.TYPE_TEXT_FLAG_MULTI_LINE;
            if (inputType != lastInputType) {
                lastInputType = inputType;
                sweetInput.setInputType(inputType);
            }
            sweetInput.setTextColor(color);
            sweetInput.setHintTextColor((color & 0x00ffffff) | 0x66000000);
            sweetInput.setHint(hint);
            sweetInput.setTextSize(TypedValue.COMPLEX_UNIT_PX, fontSize);
            int[] contentLocation = new int[2];
            View root = getWindow().getDecorView().getRootView();
            root.getLocationOnScreen(contentLocation);
            int left = Math.round(x) - contentLocation[0];
            int top = Math.round(y) + mobileSafeTopInsetPx - contentLocation[1];
            int fieldWidth = Math.max(1, Math.round(width));
            int fieldHeight = Math.max(1, Math.round(height));
            if (sweetInputPopup.isShowing()) {
                sweetInputPopup.update(left, top, fieldWidth, fieldHeight);
            } else {
                sweetInputPopup.setWidth(fieldWidth);
                sweetInputPopup.setHeight(fieldHeight);
                sweetInputPopup.showAtLocation(root, Gravity.TOP | Gravity.START, left, top);
            }
            sweetInput.setElevation(1000f);
            sweetInput.setAlpha(1f);
            sweetInput.bringToFront();
            if (sweetInput.hasFocus()) sweetInput.setSelection(sweetInput.getText().length());
        });
    }

    private static native void nativeTextChanged(String value);
    private static native void nativeViewportChanged(int height);
}
