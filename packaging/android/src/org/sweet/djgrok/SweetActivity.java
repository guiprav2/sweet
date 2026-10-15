package org.sweet.djgrok;

import android.app.NativeActivity;
import android.os.Build;
import android.os.Bundle;
import android.view.Gravity;
import android.view.View;
import android.view.WindowInsets;
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
    private int statusInsetPx;
    private int navigationInsetPx;
    private int lastSafeTopInsetPx = -1;
    private int lastSafeBottomInsetPx = -1;
    private boolean reserveSafeTop;
    private boolean reserveSafeBottom;

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
        sweetInput.setOnTouchListener((view, event) -> {
            if (event.getActionMasked() == android.view.MotionEvent.ACTION_DOWN) {
                view.requestFocus();
                view.post(() -> {
                    InputMethodManager manager = (InputMethodManager)
                            getSystemService(Context.INPUT_METHOD_SERVICE);
                    if (manager != null) {
                        manager.showSoftInput(view, InputMethodManager.SHOW_IMPLICIT);
                    }
                });
            }
            return false;
        });
        sweetInputPopup = new PopupWindow(sweetInput, 1, 1, true);
        sweetInputPopup.setFocusable(true);
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
            int top = reserveSafeTop ? statusInsetPx : 0;
            int bottom = reserveSafeBottom ? navigationInsetPx : 0;
            int fullSafeHeight = Math.max(0, root.getHeight() - top - bottom);
            int visibleSafeHeight = visible.bottom - rootLocation[1] - top
                    + (reserveSafeBottom ? 0 : navigationInsetPx);
            int height = Math.min(fullSafeHeight, Math.max(0, visibleSafeHeight));
            if (height > 0 && (height != lastViewportHeight
                    || top != lastSafeTopInsetPx || bottom != lastSafeBottomInsetPx)) {
                lastViewportHeight = height;
                lastSafeTopInsetPx = top;
                lastSafeBottomInsetPx = bottom;
                nativeViewportChanged(height, top, bottom);
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
            reserveSafeTop = reserveTop;
            reserveSafeBottom = reserveBottom;
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
                int navigationInset = insets.getSystemWindowInsetBottom();
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    topInset = insets.getInsets(WindowInsets.Type.statusBars()).top;
                    navigationInset = insets.getInsets(WindowInsets.Type.navigationBars()).bottom;
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P
                        && insets.getDisplayCutout() != null) {
                    topInset = Math.max(topInset, insets.getDisplayCutout().getSafeInsetTop());
                }
                navigationInsetPx = navigationInset;
                statusInsetPx = topInset;
                mobileSafeTopInsetPx = reserveTop ? topInset : 0;
                return insets;
            });
            content.requestApplyInsets();
        });
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
            // The EditText draws text inside its native input box so Android's
            // baseline and vertical gravity match the field geometry exactly.
            sweetInput.setTextColor(color);
            int hintAlpha = Math.round(Color.alpha(color) * 0.55f);
            sweetInput.setHintTextColor((color & 0x00ffffff) | (hintAlpha << 24));
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
    private static native void nativeViewportChanged(int height, int safeTop, int safeBottom);
}
