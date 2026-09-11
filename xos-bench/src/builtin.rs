//! The built-in suite: fifty cases across the five shapes that decide whether a
//! model can drive tools.
//!
//! Ten single tool calls, ten multi-turn sequences, ten calls carrying nested
//! object arguments, ten requiring a numeric argument read out of the prompt,
//! and ten where no tool applies and the correct move is to answer in prose.
//! The tools are system-shaped because that is the work XOS asks of a model.

use serde_json::Value;

use crate::case::{
    array_of, boolean_of, integer_of, number_of, object_of, params, string_of, tool, ArgExpect,
    Case, Category, Expect, Step,
};

fn case(id: &str, category: Category, prompt: &str, tools: Vec<Value>, expect: Expect) -> Case {
    Case {
        id: id.to_string(),
        category,
        prompt: prompt.to_string(),
        tools,
        expect,
    }
}

fn calls(steps: Vec<Step>) -> Expect {
    Expect::Sequence { steps }
}

fn one(step: Step) -> Expect {
    Expect::Sequence { steps: vec![step] }
}

pub fn suite() -> Vec<Case> {
    let mut cases = Vec::new();
    cases.extend(single_call());
    cases.extend(multi_turn());
    cases.extend(nested_args());
    cases.extend(numeric_arg());
    cases.extend(refusal());
    cases
}

// ---------------------------------------------------------------------------
// Single tool call
// ---------------------------------------------------------------------------

fn single_call() -> Vec<Case> {
    vec![
        case(
            "single-01-disk-usage",
            Category::SingleCall,
            "How much space is left on /home?",
            vec![tool(
                "disk_usage",
                "Report free and used space for a mount point",
                params(vec![("path", string_of("Mount point or directory"))], &["path"]),
            )],
            one(Step::new(
                "disk_usage",
                vec![ArgExpect::text("path", "/home")],
            )),
        ),
        case(
            "single-02-processes",
            Category::SingleCall,
            "Show me the processes using the most memory.",
            vec![tool(
                "list_processes",
                "List running processes",
                params(
                    vec![
                        ("sort_by", string_of("One of: cpu, memory, pid, name")),
                        ("limit", integer_of("How many to return")),
                    ],
                    &["sort_by"],
                ),
            )],
            one(Step::new(
                "list_processes",
                vec![ArgExpect::text("sort_by", "mem")],
            )),
        ),
        case(
            "single-03-battery",
            Category::SingleCall,
            "Is this machine running on battery right now?",
            vec![tool(
                "battery_status",
                "Report charge level and whether the machine is plugged in",
                params(vec![], &[]),
            )],
            one(Step::new("battery_status", vec![])),
        ),
        case(
            "single-04-read-file",
            Category::SingleCall,
            "Read /etc/hostname and tell me what it says.",
            vec![tool(
                "read_file",
                "Read a text file from disk",
                params(vec![("path", string_of("Absolute path"))], &["path"]),
            )],
            one(Step::new(
                "read_file",
                vec![ArgExpect::text("path", "/etc/hostname")],
            )),
        ),
        case(
            "single-05-search",
            Category::SingleCall,
            "Find any file named pacman.conf under /etc.",
            vec![tool(
                "search_files",
                "Search for files by name",
                params(
                    vec![
                        ("query", string_of("Name or glob to match")),
                        ("directory", string_of("Directory to search")),
                    ],
                    &["query", "directory"],
                ),
            )],
            one(Step::new(
                "search_files",
                vec![
                    ArgExpect::text("query", "pacman.conf"),
                    ArgExpect::text("directory", "/etc"),
                ],
            )),
        ),
        case(
            "single-06-service-status",
            Category::SingleCall,
            "Is the sshd service running?",
            vec![tool(
                "service_status",
                "Report whether a systemd unit is active",
                params(vec![("unit", string_of("Unit name"))], &["unit"]),
            )],
            one(Step::new(
                "service_status",
                vec![ArgExpect::text("unit", "sshd")],
            )),
        ),
        case(
            "single-07-network",
            Category::SingleCall,
            "What is the status of the wlan0 interface?",
            vec![tool(
                "network_status",
                "Report link state and address for an interface",
                params(vec![("interface", string_of("Interface name"))], &["interface"]),
            )],
            one(Step::new(
                "network_status",
                vec![ArgExpect::text("interface", "wlan0")],
            )),
        ),
        case(
            "single-08-package",
            Category::SingleCall,
            "Which version of ripgrep is installed?",
            vec![tool(
                "package_info",
                "Report the installed version of a package",
                params(vec![("name", string_of("Package name"))], &["name"]),
            )],
            one(Step::new(
                "package_info",
                vec![ArgExpect::text("name", "ripgrep")],
            )),
        ),
        case(
            "single-09-open-app",
            Category::SingleCall,
            "Open the terminal.",
            vec![tool(
                "open_application",
                "Launch a desktop application",
                params(vec![("name", string_of("Application name"))], &["name"]),
            )],
            one(Step::new(
                "open_application",
                vec![ArgExpect::text("name", "terminal")],
            )),
        ),
        case(
            "single-10-gpu",
            Category::SingleCall,
            "What is the GPU temperature?",
            vec![tool(
                "gpu_status",
                "Report GPU temperature, utilisation and memory use",
                params(vec![("device", integer_of("GPU index, 0 for the first"))], &[]),
            )],
            one(Step::new("gpu_status", vec![])),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Multi-turn sequences
// ---------------------------------------------------------------------------

fn multi_turn() -> Vec<Case> {
    vec![
        case(
            "multi-01-find-then-delete",
            Category::MultiTurn,
            "Find the file called old-backup.tar in /tmp and delete it.",
            vec![
                tool(
                    "search_files",
                    "Search for files by name",
                    params(
                        vec![
                            ("query", string_of("Name to match")),
                            ("directory", string_of("Directory to search")),
                        ],
                        &["query", "directory"],
                    ),
                ),
                tool(
                    "delete_file",
                    "Delete a file at an absolute path",
                    params(vec![("path", string_of("Absolute path"))], &["path"]),
                ),
            ],
            calls(vec![
                Step::returning(
                    "search_files",
                    vec![ArgExpect::text("query", "old-backup.tar")],
                    r#"{"matches":["/tmp/old-backup.tar"]}"#,
                ),
                Step::new(
                    "delete_file",
                    vec![ArgExpect::text("path", "/tmp/old-backup.tar")],
                ),
            ]),
        ),
        case(
            "multi-02-find-then-kill",
            Category::MultiTurn,
            "Something called runaway.py is pinning the CPU. Find it and stop it.",
            vec![
                tool(
                    "list_processes",
                    "List running processes",
                    params(vec![("sort_by", string_of("cpu, memory, pid or name"))], &["sort_by"]),
                ),
                tool(
                    "kill_process",
                    "Terminate a process by id",
                    params(vec![("pid", integer_of("Process id"))], &["pid"]),
                ),
            ],
            calls(vec![
                Step::returning(
                    "list_processes",
                    vec![ArgExpect::text("sort_by", "cpu")],
                    r#"{"processes":[{"pid":4821,"name":"runaway.py","cpu":98.4}]}"#,
                ),
                Step::new("kill_process", vec![ArgExpect::integer("pid", 4821)]),
            ]),
        ),
        case(
            "multi-03-search-then-install",
            Category::MultiTurn,
            "I need a terminal file manager. Search for yazi and install it.",
            vec![
                tool(
                    "package_search",
                    "Search the package repositories",
                    params(vec![("query", string_of("Search term"))], &["query"]),
                ),
                tool(
                    "package_install",
                    "Install a package by exact name",
                    params(vec![("name", string_of("Package name"))], &["name"]),
                ),
            ],
            calls(vec![
                Step::returning(
                    "package_search",
                    vec![ArgExpect::text("query", "yazi")],
                    r#"{"results":[{"name":"yazi","version":"0.3.1"}]}"#,
                ),
                Step::new("package_install", vec![ArgExpect::text("name", "yazi")]),
            ]),
        ),
        case(
            "multi-04-read-then-write",
            Category::MultiTurn,
            "Read /etc/xos/mode.conf, then write the value `supervised` to /etc/xos/mode.conf.",
            vec![
                tool(
                    "read_file",
                    "Read a text file",
                    params(vec![("path", string_of("Absolute path"))], &["path"]),
                ),
                tool(
                    "write_file",
                    "Write text to a file",
                    params(
                        vec![
                            ("path", string_of("Absolute path")),
                            ("content", string_of("Text to write")),
                        ],
                        &["path", "content"],
                    ),
                ),
            ],
            calls(vec![
                Step::returning(
                    "read_file",
                    vec![ArgExpect::text("path", "/etc/xos/mode.conf")],
                    r#"{"content":"autonomous"}"#,
                ),
                Step::new(
                    "write_file",
                    vec![
                        ArgExpect::text("path", "/etc/xos/mode.conf"),
                        ArgExpect::text("content", "supervised"),
                    ],
                ),
            ]),
        ),
        case(
            "multi-05-disk-then-clean",
            Category::MultiTurn,
            "Check the space on /var, then clear the package cache.",
            vec![
                tool(
                    "disk_usage",
                    "Report free and used space",
                    params(vec![("path", string_of("Mount point"))], &["path"]),
                ),
                tool(
                    "clear_cache",
                    "Clear a named cache",
                    params(vec![("cache", string_of("Cache name, e.g. package"))], &["cache"]),
                ),
            ],
            calls(vec![
                Step::returning(
                    "disk_usage",
                    vec![ArgExpect::text("path", "/var")],
                    r#"{"free_gb":2.1,"used_percent":94}"#,
                ),
                Step::new("clear_cache", vec![ArgExpect::text("cache", "package")]),
            ]),
        ),
        case(
            "multi-06-status-then-restart",
            Category::MultiTurn,
            "Check whether nginx is running and restart it if it is not.",
            vec![
                tool(
                    "service_status",
                    "Report whether a unit is active",
                    params(vec![("unit", string_of("Unit name"))], &["unit"]),
                ),
                tool(
                    "restart_service",
                    "Restart a systemd unit",
                    params(vec![("unit", string_of("Unit name"))], &["unit"]),
                ),
            ],
            calls(vec![
                Step::returning(
                    "service_status",
                    vec![ArgExpect::text("unit", "nginx")],
                    r#"{"unit":"nginx","active":false,"state":"failed"}"#,
                ),
                Step::new("restart_service", vec![ArgExpect::text("unit", "nginx")]),
            ]),
        ),
        case(
            "multi-07-list-then-set-resolution",
            Category::MultiTurn,
            "List the displays, then set DP-1 to 2560 by 1440.",
            vec![
                tool("list_displays", "List connected displays", params(vec![], &[])),
                tool(
                    "set_resolution",
                    "Set a display resolution",
                    params(
                        vec![
                            ("display", string_of("Display name")),
                            ("width", integer_of("Width in pixels")),
                            ("height", integer_of("Height in pixels")),
                        ],
                        &["display", "width", "height"],
                    ),
                ),
            ],
            calls(vec![
                Step::returning(
                    "list_displays",
                    vec![],
                    r#"{"displays":[{"name":"DP-1"},{"name":"HDMI-1"}]}"#,
                ),
                Step::new(
                    "set_resolution",
                    vec![
                        ArgExpect::text("display", "DP-1"),
                        ArgExpect::integer("width", 2560),
                        ArgExpect::integer("height", 1440),
                    ],
                ),
            ]),
        ),
        case(
            "multi-08-updates",
            Category::MultiTurn,
            "Check for system updates and apply them.",
            vec![
                tool("check_updates", "List available package updates", params(vec![], &[])),
                tool(
                    "apply_updates",
                    "Apply pending updates",
                    params(vec![("confirm", boolean_of("Proceed without prompting"))], &["confirm"]),
                ),
            ],
            calls(vec![
                Step::returning(
                    "check_updates",
                    vec![],
                    r#"{"pending":["linux-firmware","ripgrep"]}"#,
                ),
                Step::new("apply_updates", vec![]),
            ]),
        ),
        case(
            "multi-09-log-then-report",
            Category::MultiTurn,
            "Read the last errors from the xosd journal, then file a report titled `xosd errors`.",
            vec![
                tool(
                    "read_journal",
                    "Read recent journal entries for a unit",
                    params(
                        vec![
                            ("unit", string_of("Unit name")),
                            ("priority", string_of("Minimum priority")),
                        ],
                        &["unit"],
                    ),
                ),
                tool(
                    "file_report",
                    "File a diagnostic report",
                    params(
                        vec![
                            ("title", string_of("Report title")),
                            ("body", string_of("Report body")),
                        ],
                        &["title", "body"],
                    ),
                ),
            ],
            calls(vec![
                Step::returning(
                    "read_journal",
                    vec![ArgExpect::text("unit", "xosd")],
                    r#"{"entries":["failed to open vault","provider timeout"]}"#,
                ),
                Step::new("file_report", vec![ArgExpect::text("title", "xosd errors")]),
            ]),
        ),
        case(
            "multi-10-snapshot-then-prune",
            Category::MultiTurn,
            "List the snapshots, then delete the one called pre-update.",
            vec![
                tool("list_snapshots", "List filesystem snapshots", params(vec![], &[])),
                tool(
                    "delete_snapshot",
                    "Delete a snapshot by name",
                    params(vec![("name", string_of("Snapshot name"))], &["name"]),
                ),
            ],
            calls(vec![
                Step::returning(
                    "list_snapshots",
                    vec![],
                    r#"{"snapshots":["daily-2026-09-10","pre-update"]}"#,
                ),
                Step::new("delete_snapshot", vec![ArgExpect::text("name", "pre-update")]),
            ]),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Nested object arguments
// ---------------------------------------------------------------------------

fn nested_args() -> Vec<Case> {
    vec![
        case(
            "nested-01-event",
            Category::NestedArgs,
            "Put a meeting called Design review in the calendar for 14 October 2026 at 15:00 in room B2.",
            vec![tool(
                "create_event",
                "Create a calendar event",
                params(
                    vec![
                        ("title", string_of("Event title")),
                        (
                            "when",
                            object_of(
                                vec![
                                    ("date", string_of("ISO date, YYYY-MM-DD")),
                                    ("time", string_of("24-hour time, HH:MM")),
                                ],
                                &["date", "time"],
                                "When the event starts",
                            ),
                        ),
                        (
                            "location",
                            object_of(
                                vec![("room", string_of("Room name"))],
                                &["room"],
                                "Where the event is",
                            ),
                        ),
                    ],
                    &["title", "when"],
                ),
            )],
            one(Step::new(
                "create_event",
                vec![
                    ArgExpect::text("title", "design review"),
                    ArgExpect::object("when"),
                    ArgExpect::text("when.date", "2026-10-14"),
                    ArgExpect::text("when.time", "15:00"),
                ],
            )),
        ),
        case(
            "nested-02-backup",
            Category::NestedArgs,
            "Back up /home/shahr nightly at 02:00, skipping the Downloads folder.",
            vec![tool(
                "create_backup",
                "Define a backup job",
                params(
                    vec![
                        (
                            "target",
                            object_of(
                                vec![
                                    ("path", string_of("Directory to back up")),
                                    ("exclude", array_of(string_of("Path fragment"), "Paths to skip")),
                                ],
                                &["path"],
                                "What to back up",
                            ),
                        ),
                        (
                            "schedule",
                            object_of(
                                vec![
                                    ("frequency", string_of("hourly, daily or weekly")),
                                    ("hour", integer_of("Hour of day, 0 to 23")),
                                ],
                                &["frequency"],
                                "When to run",
                            ),
                        ),
                    ],
                    &["target", "schedule"],
                ),
            )],
            one(Step::new(
                "create_backup",
                vec![
                    ArgExpect::object("target"),
                    ArgExpect::text("target.path", "/home/shahr"),
                    ArgExpect::object("schedule"),
                    ArgExpect::text("schedule.frequency", "daily"),
                ],
            )),
        ),
        case(
            "nested-03-monitor",
            Category::NestedArgs,
            "Set the display named HDMI-1 to 1920 by 1080 at 60 hertz.",
            vec![tool(
                "configure_monitor",
                "Configure a connected display",
                params(
                    vec![
                        (
                            "display",
                            object_of(vec![("name", string_of("Connector name"))], &["name"], "Which display"),
                        ),
                        (
                            "mode",
                            object_of(
                                vec![
                                    ("width", integer_of("Pixels")),
                                    ("height", integer_of("Pixels")),
                                    ("refresh", integer_of("Hertz")),
                                ],
                                &["width", "height"],
                                "Display mode",
                            ),
                        ),
                    ],
                    &["display", "mode"],
                ),
            )],
            one(Step::new(
                "configure_monitor",
                vec![
                    ArgExpect::text("display.name", "HDMI-1"),
                    ArgExpect::integer("mode.width", 1920),
                    ArgExpect::integer("mode.height", 1080),
                    ArgExpect::integer("mode.refresh", 60),
                ],
            )),
        ),
        case(
            "nested-04-notification",
            Category::NestedArgs,
            "Send me a critical notification titled Disk full saying /var has 2 percent left.",
            vec![tool(
                "send_notification",
                "Send a desktop notification",
                params(
                    vec![
                        (
                            "message",
                            object_of(
                                vec![
                                    ("title", string_of("Headline")),
                                    ("body", string_of("Body text")),
                                ],
                                &["title", "body"],
                                "What to say",
                            ),
                        ),
                        (
                            "options",
                            object_of(
                                vec![
                                    ("urgency", string_of("low, normal or critical")),
                                    ("timeout_ms", integer_of("Dismiss after")),
                                ],
                                &["urgency"],
                                "How to present it",
                            ),
                        ),
                    ],
                    &["message"],
                ),
            )],
            one(Step::new(
                "send_notification",
                vec![
                    ArgExpect::text("message.title", "disk full"),
                    ArgExpect::any_text("message.body"),
                    ArgExpect::text("options.urgency", "critical"),
                ],
            )),
        ),
        case(
            "nested-05-vm",
            Category::NestedArgs,
            "Create a virtual machine named arch-test with 4 CPUs, 8 GB of memory and a 40 GB disk.",
            vec![tool(
                "create_vm",
                "Create a virtual machine",
                params(
                    vec![
                        ("name", string_of("VM name")),
                        (
                            "resources",
                            object_of(
                                vec![
                                    ("cpus", integer_of("Virtual CPUs")),
                                    ("memory_gb", integer_of("Memory in gigabytes")),
                                ],
                                &["cpus", "memory_gb"],
                                "Compute allocation",
                            ),
                        ),
                        (
                            "disk",
                            object_of(
                                vec![
                                    ("size_gb", integer_of("Disk size in gigabytes")),
                                    ("kind", string_of("qcow2 or raw")),
                                ],
                                &["size_gb"],
                                "Disk allocation",
                            ),
                        ),
                    ],
                    &["name", "resources", "disk"],
                ),
            )],
            one(Step::new(
                "create_vm",
                vec![
                    ArgExpect::text("name", "arch-test"),
                    ArgExpect::integer("resources.cpus", 4),
                    ArgExpect::integer("resources.memory_gb", 8),
                    ArgExpect::integer("disk.size_gb", 40),
                ],
            )),
        ),
        case(
            "nested-06-policy",
            Category::NestedArgs,
            "Add a policy that blocks reads of /home/shahr/.ssh for the whole system.",
            vec![tool(
                "set_policy",
                "Add a policy rule",
                params(
                    vec![
                        (
                            "rule",
                            object_of(
                                vec![
                                    ("action", string_of("allow, block or prompt")),
                                    ("operation", string_of("read, write or execute")),
                                    ("path", string_of("Path the rule covers")),
                                ],
                                &["action", "path"],
                                "What the rule does",
                            ),
                        ),
                        (
                            "scope",
                            object_of(
                                vec![("applies_to", string_of("user or system"))],
                                &["applies_to"],
                                "Who it covers",
                            ),
                        ),
                    ],
                    &["rule", "scope"],
                ),
            )],
            one(Step::new(
                "set_policy",
                vec![
                    ArgExpect::text("rule.action", "block"),
                    ArgExpect::text("rule.path", ".ssh"),
                    ArgExpect::text("scope.applies_to", "system"),
                ],
            )),
        ),
        case(
            "nested-07-job",
            Category::NestedArgs,
            "Schedule /usr/bin/xos-prune to run with the flag --old every day at 03:30.",
            vec![tool(
                "schedule_job",
                "Schedule a recurring command",
                params(
                    vec![
                        (
                            "command",
                            object_of(
                                vec![
                                    ("binary", string_of("Absolute path to the binary")),
                                    ("args", array_of(string_of("Argument"), "Arguments")),
                                ],
                                &["binary"],
                                "What to run",
                            ),
                        ),
                        (
                            "trigger",
                            object_of(
                                vec![
                                    ("hour", integer_of("Hour, 0 to 23")),
                                    ("minute", integer_of("Minute, 0 to 59")),
                                ],
                                &["hour", "minute"],
                                "When to run",
                            ),
                        ),
                    ],
                    &["command", "trigger"],
                ),
            )],
            one(Step::new(
                "schedule_job",
                vec![
                    ArgExpect::text("command.binary", "xos-prune"),
                    ArgExpect::array("command.args"),
                    ArgExpect::integer("trigger.hour", 3),
                    ArgExpect::integer("trigger.minute", 30),
                ],
            )),
        ),
        case(
            "nested-08-network",
            Category::NestedArgs,
            "Join the wifi network called Cavendish using WPA2 on interface wlan0.",
            vec![tool(
                "join_network",
                "Join a wireless network",
                params(
                    vec![
                        ("interface", string_of("Interface name")),
                        (
                            "network",
                            object_of(
                                vec![
                                    ("ssid", string_of("Network name")),
                                    (
                                        "security",
                                        object_of(
                                            vec![("kind", string_of("open, wpa2 or wpa3"))],
                                            &["kind"],
                                            "Security settings",
                                        ),
                                    ),
                                ],
                                &["ssid"],
                                "Which network",
                            ),
                        ),
                    ],
                    &["interface", "network"],
                ),
            )],
            one(Step::new(
                "join_network",
                vec![
                    ArgExpect::text("interface", "wlan0"),
                    ArgExpect::text("network.ssid", "cavendish"),
                    ArgExpect::text("network.security.kind", "wpa2"),
                ],
            )),
        ),
        case(
            "nested-09-goal",
            Category::NestedArgs,
            "Create a goal called Tidy downloads with two steps: list the folder, then sort by type.",
            vec![tool(
                "create_goal",
                "Create a goal with an ordered task list",
                params(
                    vec![
                        (
                            "goal",
                            object_of(
                                vec![
                                    ("title", string_of("Goal title")),
                                    ("priority", string_of("low, normal or high")),
                                ],
                                &["title"],
                                "The goal itself",
                            ),
                        ),
                        (
                            "tasks",
                            array_of(
                                object_of(
                                    vec![("description", string_of("What the step does"))],
                                    &["description"],
                                    "One step",
                                ),
                                "Ordered steps",
                            ),
                        ),
                    ],
                    &["goal", "tasks"],
                ),
            )],
            one(Step::new(
                "create_goal",
                vec![
                    ArgExpect::text("goal.title", "tidy downloads"),
                    ArgExpect::array("tasks"),
                ],
            )),
        ),
        case(
            "nested-10-memory",
            Category::NestedArgs,
            "Remember that I prefer dark themes, as a long-term preference tagged ui.",
            vec![tool(
                "remember",
                "Store something in memory",
                params(
                    vec![
                        (
                            "entry",
                            object_of(
                                vec![
                                    ("content", string_of("What to remember")),
                                    ("tier", string_of("session, working or long-term")),
                                ],
                                &["content", "tier"],
                                "The memory",
                            ),
                        ),
                        ("tags", array_of(string_of("Tag"), "Tags for retrieval")),
                    ],
                    &["entry"],
                ),
            )],
            one(Step::new(
                "remember",
                vec![
                    ArgExpect::text("entry.content", "dark"),
                    ArgExpect::text("entry.tier", "long"),
                ],
            )),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Numeric arguments
// ---------------------------------------------------------------------------

fn numeric_arg() -> Vec<Case> {
    vec![
        case(
            "numeric-01-volume",
            Category::NumericArg,
            "Set the volume to 40 percent.",
            vec![tool(
                "set_volume",
                "Set output volume",
                params(vec![("level", integer_of("Percent, 0 to 100"))], &["level"]),
            )],
            one(Step::new("set_volume", vec![ArgExpect::integer("level", 40)])),
        ),
        case(
            "numeric-02-brightness",
            Category::NumericArg,
            "Dim the screen to 35 percent.",
            vec![tool(
                "set_brightness",
                "Set display brightness",
                params(vec![("percent", integer_of("Percent, 0 to 100"))], &["percent"]),
            )],
            one(Step::new(
                "set_brightness",
                vec![ArgExpect::integer("percent", 35)],
            )),
        ),
        case(
            "numeric-03-resize",
            Category::NumericArg,
            "Resize photo.jpg to 1920 pixels wide.",
            vec![tool(
                "resize_image",
                "Resize an image",
                params(
                    vec![
                        ("path", string_of("Image path")),
                        ("width", integer_of("Target width in pixels")),
                    ],
                    &["path", "width"],
                ),
            )],
            one(Step::new(
                "resize_image",
                vec![
                    ArgExpect::text("path", "photo.jpg"),
                    ArgExpect::integer("width", 1920),
                ],
            )),
        ),
        case(
            "numeric-04-sleep",
            Category::NumericArg,
            "Suspend the machine after 30 minutes of idling.",
            vec![tool(
                "set_idle_suspend",
                "Suspend after an idle period",
                params(vec![("minutes", integer_of("Idle minutes"))], &["minutes"]),
            )],
            one(Step::new(
                "set_idle_suspend",
                vec![ArgExpect::integer("minutes", 30)],
            )),
        ),
        case(
            "numeric-05-cpu-limit",
            Category::NumericArg,
            "Cap the indexer at 75 percent of one core.",
            vec![tool(
                "limit_cpu",
                "Cap CPU use for a service",
                params(
                    vec![
                        ("service", string_of("Service name")),
                        ("percent", integer_of("Percent of one core")),
                    ],
                    &["service", "percent"],
                ),
            )],
            one(Step::new(
                "limit_cpu",
                vec![ArgExpect::integer("percent", 75)],
            )),
        ),
        case(
            "numeric-06-timeout",
            Category::NumericArg,
            "Set the provider timeout to 120 seconds.",
            vec![tool(
                "set_timeout",
                "Set a request timeout",
                params(vec![("seconds", integer_of("Timeout in seconds"))], &["seconds"]),
            )],
            one(Step::new(
                "set_timeout",
                vec![ArgExpect::integer("seconds", 120)],
            )),
        ),
        case(
            "numeric-07-vram",
            Category::NumericArg,
            "Reserve 512 megabytes of VRAM for the model cache.",
            vec![tool(
                "reserve_vram",
                "Reserve video memory",
                params(vec![("megabytes", integer_of("Megabytes to reserve"))], &["megabytes"]),
            )],
            one(Step::new(
                "reserve_vram",
                vec![ArgExpect::integer("megabytes", 512)],
            )),
        ),
        case(
            "numeric-08-fan",
            Category::NumericArg,
            "Hold the GPU fan at 1200 rpm.",
            vec![tool(
                "set_fan_speed",
                "Set a fixed fan speed",
                params(vec![("rpm", integer_of("Revolutions per minute"))], &["rpm"]),
            )],
            one(Step::new("set_fan_speed", vec![ArgExpect::integer("rpm", 1200)])),
        ),
        case(
            "numeric-09-scale",
            Category::NumericArg,
            "Set the display scaling factor to 1.5.",
            vec![tool(
                "set_scale",
                "Set display scaling",
                params(vec![("factor", number_of("Scale factor, e.g. 1.0 or 1.5"))], &["factor"]),
            )],
            one(Step::new("set_scale", vec![ArgExpect::number("factor", 1.5)])),
        ),
        case(
            "numeric-10-snapshots",
            Category::NumericArg,
            "Keep only the last 7 snapshots.",
            vec![tool(
                "set_snapshot_retention",
                "Set how many snapshots to keep",
                params(vec![("count", integer_of("Snapshots to retain"))], &["count"]),
            )],
            one(Step::new(
                "set_snapshot_retention",
                vec![ArgExpect::integer("count", 7)],
            )),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Correct refusal to call
// ---------------------------------------------------------------------------

fn refusal() -> Vec<Case> {
    let volume = || {
        tool(
            "set_volume",
            "Set output volume",
            params(vec![("level", integer_of("Percent, 0 to 100"))], &["level"]),
        )
    };
    let disk = || {
        tool(
            "disk_usage",
            "Report free and used space",
            params(vec![("path", string_of("Mount point"))], &["path"]),
        )
    };
    let kill = || {
        tool(
            "kill_process",
            "Terminate a process by id",
            params(vec![("pid", integer_of("Process id"))], &["pid"]),
        )
    };
    let read = || {
        tool(
            "read_file",
            "Read a text file",
            params(vec![("path", string_of("Absolute path"))], &["path"]),
        )
    };
    let install = || {
        tool(
            "package_install",
            "Install a package",
            params(vec![("name", string_of("Package name"))], &["name"]),
        )
    };

    vec![
        case(
            "refusal-01-general-knowledge",
            Category::Refusal,
            "What is the capital of Portugal?",
            vec![volume(), disk()],
            Expect::NoCall,
        ),
        case(
            "refusal-02-creative",
            Category::Refusal,
            "Write me a two-line poem about rain on a window.",
            vec![disk()],
            Expect::NoCall,
        ),
        case(
            "refusal-03-explain",
            Category::Refusal,
            "Explain what a zombie process is, in two sentences.",
            vec![kill()],
            Expect::NoCall,
        ),
        case(
            "refusal-04-concept",
            Category::Refusal,
            "What does RAID 5 protect against, and what does it not?",
            vec![read(), disk()],
            Expect::NoCall,
        ),
        case(
            "refusal-05-closing",
            Category::Refusal,
            "That is everything I needed, thanks.",
            vec![volume(), read()],
            Expect::NoCall,
        ),
        case(
            "refusal-06-compare",
            Category::Refusal,
            "What is the difference between apt and pacman as package managers?",
            vec![install()],
            Expect::NoCall,
        ),
        case(
            "refusal-07-trivia",
            Category::Refusal,
            "How many time zones does Russia span?",
            vec![
                tool(
                    "set_timezone",
                    "Set the system timezone",
                    params(vec![("zone", string_of("IANA zone name"))], &["zone"]),
                ),
                disk(),
            ],
            Expect::NoCall,
        ),
        case(
            "refusal-08-definition",
            Category::Refusal,
            "What does it mean for an operation to be idempotent?",
            vec![
                tool(
                    "restart_service",
                    "Restart a systemd unit",
                    params(vec![("unit", string_of("Unit name"))], &["unit"]),
                ),
                read(),
            ],
            Expect::NoCall,
        ),
        case(
            "refusal-09-greeting",
            Category::Refusal,
            "Morning. How are you doing today?",
            vec![
                tool(
                    "search_files",
                    "Search for files by name",
                    params(
                        vec![
                            ("query", string_of("Name to match")),
                            ("directory", string_of("Directory")),
                        ],
                        &["query", "directory"],
                    ),
                ),
                volume(),
            ],
            Expect::NoCall,
        ),
        case(
            "refusal-10-opinion",
            Category::Refusal,
            "Do you think tiling window managers are worth learning?",
            vec![install(), volume()],
            Expect::NoCall,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ships_fifty_cases() {
        assert_eq!(suite().len(), 50);
    }

    #[test]
    fn covers_every_category_evenly() {
        let cases = suite();
        for category in Category::all() {
            let count = cases.iter().filter(|c| c.category == category).count();
            assert_eq!(count, 10, "{} has {} cases", category.label(), count);
        }
    }

    #[test]
    fn ids_are_unique() {
        let cases = suite();
        let ids: HashSet<&str> = cases.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), cases.len());
    }

    #[test]
    fn every_expected_tool_is_declared() {
        for case in suite() {
            for step in case.steps() {
                let declared = case
                    .tools
                    .iter()
                    .any(|t| t.pointer("/function/name").and_then(Value::as_str) == Some(step.tool.as_str()));
                assert!(declared, "{} expects undeclared tool {}", case.id, step.tool);
            }
        }
    }

    #[test]
    fn multi_turn_cases_have_at_least_two_steps() {
        for case in suite().iter().filter(|c| c.category == Category::MultiTurn) {
            assert!(case.steps().len() >= 2, "{} is not multi-turn", case.id);
        }
    }

    #[test]
    fn refusal_cases_expect_no_call() {
        for case in suite().iter().filter(|c| c.category == Category::Refusal) {
            assert!(matches!(case.expect, Expect::NoCall), "{} should expect no call", case.id);
        }
    }

    #[test]
    fn every_case_serialises() {
        let text = serde_json::to_string(&suite()).expect("suite serialises");
        let parsed: Vec<Case> = serde_json::from_str(&text).expect("suite round-trips");
        assert_eq!(parsed.len(), 50);
    }
}
