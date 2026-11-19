use ./core.nu [
    ISSUE_KINDS
    MAINTENANCE_FITS
    CONFIDENCE_LEVELS
    PRIORITY_LABELS
    DECISIONS
    IMPLEMENTATIONS
    IMPLEMENTATION_RESULTS
    REASON_MAX_LENGTH
    TITLE_MAX_LENGTH
    BODY_MAX_LENGTH
]

export def implementation-schema []: nothing -> record {
    {
        "type": "object"
        "additionalProperties": false
        "required": [
            "implementation"
            "title"
            "body"
        ]
        "properties": {
            "implementation": {"type": "string", "enum": $IMPLEMENTATION_RESULTS}
            "title": {"type": "string", "maxLength": $TITLE_MAX_LENGTH}
            "body": {"type": "string", "maxLength": $BODY_MAX_LENGTH}
        }
        "oneOf": [
            {
                "properties": {
                    "implementation": {"const": "prepared"}
                    "title": {"minLength": 1, "pattern": '^[^\r\n]*\S[^\r\n]*$'}
                    "body": {"minLength": 1, "pattern": '\S'}
                }
            }
            {
                "properties": {
                    "implementation": {"const": "none"}
                    "title": {"maxLength": 0}
                    "body": {"maxLength": 0}
                }
            }
        ]
    }
}

export def issue-schema []: nothing -> record {
    {
        "type": "object"
        "additionalProperties": false
        "required": [
            "kind"
            "maintenance_fit"
            "confidence"
            "decision"
            "priority"
            "implementation"
            "reason"
        ]
        "properties": {
            "kind": {"type": "string", "enum": $ISSUE_KINDS}
            "maintenance_fit": {"type": "string", "enum": $MAINTENANCE_FITS}
            "confidence": {"type": "string", "enum": $CONFIDENCE_LEVELS}
            "decision": {"type": "string", "enum": $DECISIONS}
            "priority": {"type": "string", "enum": $PRIORITY_LABELS}
            "implementation": {"type": "string", "enum": $IMPLEMENTATIONS}
            "reason": {"type": "string", "minLength": 1, "maxLength": $REASON_MAX_LENGTH}
        }
    }
}

export def pr-schema []: nothing -> record {
    {
        "type": "object"
        "additionalProperties": false
        "required": [
            "decision"
            "reason"
        ]
        "properties": {
            "decision": {"type": "string", "enum": $DECISIONS}
            "reason": {"type": "string", "minLength": 1, "maxLength": $REASON_MAX_LENGTH}
        }
    }
}
