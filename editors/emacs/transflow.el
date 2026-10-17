;;; transflow.el --- Local dataset LSP alongside Python tooling -*- lexical-binding: t; -*-
;; Package-Requires: ((emacs "29.1"))
;; No package installation, source evaluation, telemetry or networking.
;;; Commentary:
;; Explicit M-x transflow-mode uses the matched local CLI.  This additive minor
;; mode leaves Eglot/lsp-mode/Python CAPF providers enabled; server owns semantics.
;;; Code:
(require 'cl-lib)
(require 'jsonrpc)
(require 'url-util)
(require 'xref)
(require 'flymake)
(eval-when-compile (require 'python))
(declare-function python-shell-get-process "python")
(declare-function python-shell-send-string-no-output "python")
(defgroup transflow nil "Local Transflow dataset references." :group 'languages)
(defcustom transflow-executable "transflow" "Matched native CLI executable." :type 'string)
(defcustom transflow-python nil "Optional Python 3.14 syntax interpreter." :type '(choice (const nil) file))
(defcustom transflow-branch nil "Optional explicit data branch." :type '(choice (const nil) string))
(defvar transflow--connections (make-hash-table :test #'equal))
(defvar transflow-mode nil)
(defvar-local transflow--connection nil)
(defvar-local transflow--version 0)
(defvar-local transflow--reported nil)
(defvar-local transflow--diagnostics nil)
(defvar-local transflow--timer nil)
(defun transflow--uri (path)
  "Encode local PATH as a file URI."
  (concat "file://" (url-hexify-string (expand-file-name path) (cons ?/ url-unreserved-chars))))
(defun transflow--root ()
  "Find the explicit local workspace boundary."
  (or (and buffer-file-name (not (file-remote-p buffer-file-name))
           (locate-dominating-file buffer-file-name "workspace.toml"))
      (user-error "Open a local file inside a Transflow workspace")))
(defun transflow--position (&optional point)
  "Encode POINT in UTF-16 LSP coordinates."
  (save-excursion
    (when point (goto-char point))
    (list :line (1- (line-number-at-pos))
          :character (/ (length (encode-coding-string
                               (buffer-substring-no-properties (line-beginning-position) (point))
                               'utf-16le t)) 2))))
(defun transflow--point (position)
  "Decode LSP POSITION in the current buffer."
  (save-excursion
    (goto-char (point-min)) (forward-line (plist-get position :line))
    (let ((units (plist-get position :character)))
      (while (and (> units 0) (< (point) (line-end-position)))
        (setq units (- units (if (> (char-after) #xffff) 2 1)))
        (forward-char)))
    (point)))
(defun transflow--notification (_connection method params)
  "Receive diagnostics for the exact local document identified in PARAMS."
  (when (eq method 'textDocument/publishDiagnostics)
    (dolist (buffer (buffer-list))
      (with-current-buffer buffer
        (when (and transflow-mode buffer-file-name
                   (equal (plist-get params :uri) (transflow--uri buffer-file-name))
                   (or (null (plist-get params :version))
                       (= transflow--version (plist-get params :version))))
          (setq transflow--diagnostics (plist-get params :diagnostics))
          (when transflow--reported (transflow--flymake transflow--reported)))))))
(defun transflow--start (root)
  "Start one local server for ROOT; never interpolate paths into a shell."
  (let* ((name (concat "transflow-" (md5 root)))
         (connection
          (make-instance 'jsonrpc-process-connection
                         :name name :events-buffer-scrollback-size 0
                         :notification-dispatcher #'transflow--notification
                         :process (lambda ()
                                    (make-process
                                     :name name :connection-type 'pipe :noquery t
                                     :coding 'utf-8-emacs-unix
                                     :stderr (get-buffer-create (format "*%s stderr*" name))
                                     :command (append (list transflow-executable "--workspace" root "lsp")
                                                      (when transflow-python (list "--python" transflow-python))
                                                      (when transflow-branch (list "--branch" transflow-branch))))))))
    (condition-case error
        (progn
          (jsonrpc-request connection 'initialize
                           (list :processId (emacs-pid) :rootUri (transflow--uri root)
                                 :workspaceFolders (vector (list :uri (transflow--uri root) :name root))
                                 :capabilities (make-hash-table)) :timeout 10)
          (jsonrpc-notify connection 'initialized (make-hash-table))
          connection)
      (error (jsonrpc-shutdown connection) (signal (car error) (cdr error))))))
(defun transflow--sync ()
  "Send a full draft; parsing is debounced by the native server."
  (when (and transflow-mode transflow--connection (jsonrpc-running-p transflow--connection))
    (cl-incf transflow--version)
    (jsonrpc-notify transflow--connection 'textDocument/didChange
                    (list :textDocument (list :uri (transflow--uri buffer-file-name) :version transflow--version)
                          :contentChanges (vector (list :text (buffer-substring-no-properties (point-min) (point-max))))))))
(defun transflow--changed (&rest _)
  "Schedule one draft update after a short typing pause."
  (when (timerp transflow--timer) (cancel-timer transflow--timer))
  (let ((buffer (current-buffer)))
    (setq transflow--timer (run-with-idle-timer 0.15 nil
                                               (lambda () (when (buffer-live-p buffer)
                                                            (with-current-buffer buffer (transflow--sync))))))))
(defun transflow--request (method)
  "Request the dataset feature METHOD at point."
  (when (timerp transflow--timer) (cancel-timer transflow--timer))
  (transflow--sync)
  (jsonrpc-request transflow--connection method
                   (list :textDocument (list :uri (transflow--uri buffer-file-name))
                         :position (transflow--position)) :timeout 10))
(defun transflow-completion-at-point ()
  "Supply server-selected dataset completions alongside ordinary Python CAPFs."
  (when transflow-mode
    (let* ((result (transflow--request 'textDocument/completion))
           (items (append (plist-get result :items) nil))
           (edit (plist-get (car items) :textEdit))
           (range (plist-get edit :range)))
      (when range
        (list (transflow--point (plist-get range :start))
              (transflow--point (plist-get range :end))
              (mapcar (lambda (i) (plist-get i :label)) items) :exclusive 'no)))))
(defun transflow-eldoc (callback &rest _)
  "Show server hover using Eldoc CALLBACK."
  (when transflow-mode
    (let ((hover (transflow--request 'textDocument/hover)))
      (when hover (funcall callback (plist-get (plist-get hover :contents) :value)) t))))
(defun transflow--flymake (report &rest _)
  "Report only this service's diagnostics using REPORT."
  (setq transflow--reported report)
  (funcall report
           (mapcar (lambda (d)
                     (let ((range (plist-get d :range)))
                       (flymake-make-diagnostic
                        (current-buffer) (transflow--point (plist-get range :start))
                        (transflow--point (plist-get range :end))
                        (pcase (plist-get d :severity) (1 :error) (2 :warning) (_ :note))
                        (plist-get d :message))))
                   (append transflow--diagnostics nil))))
(defun transflow-open-producer ()
  "Open the server-validated local producer at point."
  (interactive)
  (let* ((location (transflow--request 'textDocument/definition))
         (uri (plist-get location :uri)))
    (unless (and uri (string-prefix-p "file:///" uri)) (user-error "No retained local producer location"))
    (xref-push-marker-stack)
    (find-file (decode-coding-string (url-unhex-string (substring uri 7)) 'utf-8))
    (goto-char (transflow--point (plist-get (plist-get location :range) :start)))))
;;;###autoload
(defun transflow-python-shell-compat (&optional process)
  "Use Python 3.14 file evaluation in an existing Python PROCESS.
This explicit comint setup replaces deprecated `codecs.open' calls with `open',
preserving encodings and newline handling.  It does not evaluate a user buffer
or change the installed editor.  Ordinary Python LSP clients do not need it."
  (interactive)
  (require 'python)
  (let ((code python-shell-eval-file-setup-code))
    (when (string-match-p "codecs\\.open" code)
      (setq code (string-replace "codecs.open(" "open(" code)
            code (string-replace "encoding='latin-1')" "encoding='latin-1', newline='')" code)
            code (string-replace "encoding=encoding)" "encoding=encoding, newline='')" code))
      ;; A quoted single-line bootstrap avoids invoking the legacy file helper.
      (python-shell-send-string-no-output
       (concat "exec(" (json-serialize code) ")")
       (or process (python-shell-get-process) (user-error "Start a Python shell first"))))))
(defun transflow-stop ()
  "Stop all explicitly started Transflow editor processes."
  (interactive)
  (maphash (lambda (_root connection)
             (when (jsonrpc-running-p connection)
               (ignore-errors (jsonrpc-request connection 'shutdown nil :timeout 2)
                              (jsonrpc-notify connection 'exit nil))
               (jsonrpc-shutdown connection))) transflow--connections)
  (clrhash transflow--connections))
(defun transflow--close ()
  "Close this buffer without stopping servers shared by another buffer."
  (when (timerp transflow--timer) (cancel-timer transflow--timer))
  (when (and transflow--connection (jsonrpc-running-p transflow--connection))
    (jsonrpc-notify transflow--connection 'textDocument/didClose
                    (list :textDocument (list :uri (transflow--uri buffer-file-name)))))
  (setq transflow--connection nil))
;;;###autoload
(define-minor-mode transflow-mode
  "Add local Transflow dataset features without replacing Python tooling."
  :lighter " TF"
  (if transflow-mode
      (let* ((root (file-truename (transflow--root)))
             (connection (gethash root transflow--connections)))
        (unless (and connection (jsonrpc-running-p connection))
          (setq connection (transflow--start root))
          (puthash root connection transflow--connections))
        (setq transflow--connection connection transflow--version 1)
        (jsonrpc-notify connection 'textDocument/didOpen
                        (list :textDocument (list :uri (transflow--uri buffer-file-name)
                                                  :languageId "python" :version 1
                                                  :text (buffer-substring-no-properties (point-min) (point-max)))))
        (add-hook 'completion-at-point-functions #'transflow-completion-at-point nil t)
        (add-hook 'eldoc-documentation-functions #'transflow-eldoc nil t)
        (add-hook 'flymake-diagnostic-functions #'transflow--flymake nil t)
        (add-hook 'after-change-functions #'transflow--changed nil t)
        (add-hook 'kill-buffer-hook #'transflow--close nil t))
    (transflow--close)
    (remove-hook 'completion-at-point-functions #'transflow-completion-at-point t)
    (remove-hook 'eldoc-documentation-functions #'transflow-eldoc t)
    (remove-hook 'flymake-diagnostic-functions #'transflow--flymake t)
    (remove-hook 'after-change-functions #'transflow--changed t)
    (remove-hook 'kill-buffer-hook #'transflow--close t)))
(add-hook 'kill-emacs-hook #'transflow-stop)
(provide 'transflow)
;;; transflow.el ends here
