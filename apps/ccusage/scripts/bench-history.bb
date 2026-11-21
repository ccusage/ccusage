#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped nixpkgs#bash nixpkgs#time nixpkgs#hyperfine nixpkgs#git --command bb

(ns bench-history
  (:require [babashka.classpath :as cp]
            [babashka.fs :as fs]))

(cp/add-classpath (str (fs/parent (fs/absolutize *file*))))

(require '[compare-pr-performance.history :as history])

(when (= *file* (System/getProperty "babashka.file"))
  (try
    (apply history/history-main *command-line-args*)
    (catch Exception error
      (binding [*out* *err*]
        (println (ex-message error)))
      (System/exit 1))))
