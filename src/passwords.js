// BrowseGoo's script for saved passwords, which runs in the top frame of each
// page, in a JavaScript world of its own. The page's scripts share the page
// with it, but not its variables or the handler it sends messages to, and they
// can't change the built-in objects that it uses.
//
// It tells BrowseGoo when a password is entered in a form, and when a username
// or password field is clicked. It only fills in fields when BrowseGoo calls
// `browsegooFill`, which it does only when the user chooses a saved password.
(() => {
  "use strict";

  const send = (message) => {
    const handler = window.webkit?.messageHandlers?.browsegooPasswords;
    if (handler) {
      handler.postMessage(message);
    }
  };

  const TEXT_TYPES = new Set(["text", "email", "tel"]);

  const isInput = (element) => element instanceof HTMLInputElement;
  const isPassword = (element) => isInput(element) && element.type === "password";
  const isText = (element) => isInput(element) && TEXT_TYPES.has(element.type);
  const isShown = (element) => element.getClientRects().length > 0;
  const canFill = (element) => !element.disabled && !element.readOnly;

  // The inputs of the field's form, or of the page if it isn't in one, in order.
  const inputsAround = (field) =>
    Array.from(field.form ? field.form.elements : document.querySelectorAll("input")).filter(
      isInput
    );

  // The username field for a password field: one that says it's for a
  // username, or else the last text field shown before it.
  const usernameFieldFor = (passwordField) => {
    const inputs = inputsAround(passwordField);
    const named = inputs.find(
      (input) => isText(input) && /\busername\b/.test(input.autocomplete || "")
    );
    if (named) {
      return named;
    }
    const before = inputs.slice(0, inputs.indexOf(passwordField));
    return before.filter((input) => isText(input) && isShown(input)).pop() || null;
  };

  // The password field for a username field: the first one after it.
  const passwordFieldFor = (usernameField) => {
    const inputs = inputsAround(usernameField);
    const after = inputs.slice(inputs.indexOf(usernameField) + 1);
    return after.find(isPassword) || inputs.find(isPassword) || null;
  };

  const isUsernameField = (field) =>
    isText(field) &&
    inputsAround(field).some((input) => isPassword(input) && usernameFieldFor(input) === field);

  // Tells BrowseGoo about the password entered in the form that `field` is
  // in, which is the last one filled in, as with a new password typed twice.
  const reportEntered = (field) => {
    const passwordField = inputsAround(field)
      .filter((input) => isPassword(input) && input.value !== "")
      .pop();
    if (!passwordField) {
      return;
    }
    const usernameField = usernameFieldFor(passwordField);
    send({
      kind: "entered",
      host: location.hostname,
      username: usernameField ? usernameField.value : "",
      password: passwordField.value,
    });
  };

  // When a password is typed and the field is left, or the form is sent.
  document.addEventListener(
    "change",
    (event) => {
      if (event.isTrusted && isPassword(event.target)) {
        reportEntered(event.target);
      }
    },
    true
  );
  document.addEventListener(
    "submit",
    (event) => {
      if (event.target instanceof HTMLFormElement) {
        const first = Array.from(event.target.elements).find(isPassword);
        if (first) {
          reportEntered(first);
        }
      }
    },
    true
  );

  // The username or password field that was last clicked, for filling in.
  let clickedField = null;

  document.addEventListener(
    "click",
    (event) => {
      const field = event.target;
      if (!event.isTrusted || !canFill(field) || !(isPassword(field) || isUsernameField(field))) {
        return;
      }
      clickedField = field;
      const box = field.getBoundingClientRect();
      send({
        kind: "clicked",
        host: location.hostname,
        x: box.x,
        y: box.y,
        width: box.width,
        height: box.height,
      });
    },
    true
  );

  const fill = (field, value) => {
    if (!field || !canFill(field)) {
      return;
    }
    field.focus();
    field.value = value;
    // So that the page's own scripts see the change, as when typing.
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.dispatchEvent(new Event("change", { bubbles: true }));
  };

  // Fills in the fields of the form whose field was last clicked, if the
  // page is still on `host`.
  globalThis.browsegooFill = (host, username, password) => {
    const field = clickedField;
    if (location.hostname !== host || !field || !field.isConnected) {
      return false;
    }
    const passwordField = isPassword(field) ? field : passwordFieldFor(field);
    const usernameField = isPassword(field) ? usernameFieldFor(field) : field;
    if (usernameField && username !== "") {
      fill(usernameField, username);
    }
    fill(passwordField, password);
    return true;
  };
})();
