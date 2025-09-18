# BlockTree Redesign: From TagSpec to AST

## Problem Statement

The current Django BlockTree implementation suffers from several architectural issues:

1. **Unnecessary intermediate layer**: `TagShape` mirrors `TagSpec` without adding real value - it's just another translation layer that adds complexity
2. **Re-deriving logic**: The `BlockTreeBuilder` repeatedly re-derives information that should be encapsulated (checking if tags are openers/closers/intermediates)
3. **Hardcoded special cases**: Tags like `blocktrans`, `cache`, and `filter` that need argument matching are handled via hardcoded `MustMatchOpenName` policy
4. **Poor separation of concerns**: The builder does too much work that should be delegated to the shapes
5. **Not leveraging Rust's type system**: Missing opportunities to use rich enums and pattern matching to make invalid states unrepresentable

## Design Philosophy

The redesign follows these principles:

1. **Data-driven, not code-driven**: The behavior should emerge from the data structure, not from hardcoded logic
2. **Boring is good**: Verbose but obvious is better than clever and confusing  
3. **Single source of truth**: Define behavior once in the spec, use it everywhere
4. **Rich types**: Use Rust's type system to encode domain knowledge
5. **Pre-compute everything**: Build indices once, query them efficiently

## The Key Insight

Django template tags that need argument matching at close (like `{% block sidebar %}...{% endblock sidebar %}`) already have a consistent pattern: **the end tag can take arguments**. Instead of inventing a special "policy" field, we just describe what arguments the end tag accepts.

If the end tag has arguments defined, those arguments need to match between opener and closer. If not, it's a simple close. The behavior emerges from the structure.

## Architecture Overview

```
TagSpecDef (djls-conf)           # User-facing YAML/JSON configuration
    ↓
TagSpec (djls-semantic)          # Internal representation 
    ↓
TagShapes (djls-semantic)        # Rich connective layer with pre-computed indices
    ↓
BlockTree                        # Self-building AST structure using shapes
```

**Key simplification**: No separate BlockTreeBuilder! The BlockTree builds itself using the TagShapes' knowledge.

## Layer-by-Layer Design

### Layer 1: TagSpecDef (djls-conf/src/tagspecs.rs)

**Status**: ✅ Already perfect! No changes needed.

```rust
pub struct EndTagDef {
    pub name: String,
    pub optional: bool,
    pub args: Vec<TagArgDef>,  // ← This drives the matching behavior!
}
```

Example user configuration:
```yaml
# Simple block tag (no argument matching)
- name: for
  end_tag:
    name: endfor
    # No args = simple close

# Block tag with name matching
- name: block
  args:
    - name: block_name
      type: literal
      required: true
  end_tag:
    name: endblock
    args:
      - name: block_name
        type: literal
        required: false  # Optional, but must match if provided

# Tag with selective argument matching
- name: cache
  args:
    - name: timeout
      type: number
      required: true
    - name: cache_key
      type: string
      required: true
  end_tag:
    name: endcache
    args:
      - name: cache_key  # Only this arg needs to match
        type: string
        required: false
```

### Layer 2: TagSpec (djls-semantic/src/templatetags/specs.rs)

**Status**: ✅ Already perfect! No changes needed.

The `EndTag` structure already has `args: L<TagArg>` and the conversion from `TagSpecDef` → `TagSpec` already handles this correctly.

### Layer 3: TagShape/TagShapes (djls-semantic/src/blocks/shapes.rs)

**Status**: 🔧 Complete rewrite needed

This is where the magic happens. TagShapes becomes the "brain" that understands all tag relationships and provides O(1) lookups for the builder.

```rust
pub struct TagShapes {
    // Primary shape storage
    shapes: FxHashMap<String, TagShape>,
    
    // Pre-computed reverse indices for O(1) lookups
    closer_to_opener: FxHashMap<String, String>,
    intermediate_to_openers: FxHashMap<String, Vec<String>>,
}

pub enum TagShape {
    Leaf {
        name: String,
    },
    Block {
        name: String,
        end: EndShape,
        intermediates: Vec<IntermediateShape>,
    },
}

pub struct EndShape {
    name: String,
    optional: bool,
    match_args: Vec<MatchArg>,  // Derived from end_tag.args
}

pub struct MatchArg {
    name: String,
    arg_type: ArgType,
    required: bool,  // Must this arg be present at close?
    opener_position: Option<usize>,  // Pre-computed position in opener's args
}
```

Key methods:

```rust
impl TagShapes {
    /// Build from TagSpecs, pre-computing all indices
    pub fn from_specs(specs: &TagSpecs) -> Self {
        let mut shapes = FxHashMap::default();
        let mut closer_to_opener = FxHashMap::default();
        let mut intermediate_to_openers = FxHashMap::default();
        
        for (name, spec) in specs {
            let shape = TagShape::from_spec(name, spec);
            
            // Build reverse indices
            match &shape {
                TagShape::Block { end, intermediates, .. } => {
                    // Map closer -> opener
                    closer_to_opener.insert(end.name.clone(), name.clone());
                    
                    // Map each intermediate -> [openers that allow it]
                    for inter in intermediates {
                        intermediate_to_openers
                            .entry(inter.name.clone())
                            .or_default()
                            .push(name.clone());
                    }
                }
                TagShape::Leaf { .. } => {}
            }
            
            shapes.insert(name.clone(), shape);
        }
        
        Self { shapes, closer_to_opener, intermediate_to_openers }
    }
    
    /// What kind of tag is this? O(1) lookup
    pub fn classify(&self, tag_name: &str) -> TagClass {
        if let Some(shape) = self.shapes.get(tag_name) {
            return TagClass::Opener { shape: shape.clone() };
        }
        if let Some(opener) = self.closer_to_opener.get(tag_name) {
            return TagClass::Closer { 
                opener_name: opener.clone(),
            };
        }
        if let Some(openers) = self.intermediate_to_openers.get(tag_name) {
            return TagClass::Intermediate { 
                possible_openers: openers.clone(),
            };
        }
        TagClass::Unknown
    }
    
    /// Validate a close tag against its opener
    pub fn validate_close(
        &self,
        opener_name: &str,
        opener_bits: &[TagBit],
        closer_bits: &[TagBit],
    ) -> CloseValidation {
        let shape = match self.shapes.get(opener_name) {
            Some(s) => s,
            None => return CloseValidation::NotABlock,
        };
        
        match shape {
            TagShape::Block { end, .. } => {
                // No args to match? Simple close
                if end.match_args.is_empty() {
                    return CloseValidation::Valid;
                }
                
                // Validate each arg that should match
                for match_arg in &end.match_args {
                    let opener_val = extract_arg_value(opener_bits, match_arg);
                    let closer_val = extract_arg_value(closer_bits, match_arg);
                    
                    match (opener_val, closer_val, match_arg.required) {
                        (Some(o), Some(c), _) if o != c => {
                            return CloseValidation::ArgumentMismatch {
                                arg: match_arg.name.clone(),
                                expected: o,
                                got: c,
                            };
                        }
                        (Some(o), None, true) => {
                            return CloseValidation::MissingRequiredArg {
                                arg: match_arg.name.clone(),
                                expected: o,
                            };
                        }
                        (None, Some(c), _) if match_arg.required => {
                            return CloseValidation::UnexpectedArg {
                                arg: match_arg.name.clone(),
                                got: c,
                            };
                        }
                        _ => continue,
                    }
                }
                CloseValidation::Valid
            }
            TagShape::Leaf { .. } => CloseValidation::NotABlock,
        }
    }
    
    /// Can this intermediate appear in the current context?
    pub fn is_valid_intermediate(&self, inter_name: &str, opener_name: &str) -> bool {
        self.intermediate_to_openers
            .get(inter_name)
            .map(|openers| openers.contains(&opener_name.to_string()))
            .unwrap_or(false)
    }
}

pub enum TagClass {
    Opener { shape: TagShape },
    Closer { opener_name: String },
    Intermediate { possible_openers: Vec<String> },
    Unknown,
}

pub enum CloseValidation {
    Valid,
    NotABlock,
    ArgumentMismatch { arg: String, expected: String, got: String },
    MissingRequiredArg { arg: String, expected: String },
    UnexpectedArg { arg: String, got: String },
}
```

### Layer 4: BlockTree (djls-semantic/src/blocks/tree.rs)

**Status**: 🔧 Merge builder into BlockTree itself

Instead of a separate builder, BlockTree becomes self-building:

```rust
pub struct BlockTree {
    roots: Vec<BlockId>,
    blocks: Blocks,
    // Transient state during building (cleared after finish())
    stack: Vec<TreeFrame>,
}

struct TreeFrame {
    opener_name: String,
    opener_bits: Vec<TagBit>,  // Store for validation
    opener_span: Span,
    container_body: BlockId,
    segment_body: BlockId,
    parent_body: BlockId,
}

impl BlockTree {
    pub fn new() -> Self {
        let (blocks, root) = Blocks::with_root();
        Self {
            roots: vec![root],
            blocks,
            stack: Vec::new(),
        }
    }
    
    /// Build the tree from a nodelist
    pub fn build(db: &dyn Db, nodelist: NodeList, shapes: &TagShapes) -> Self {
        let mut tree = BlockTree::new();
        
        for node in nodelist.nodelist(db).iter().cloned() {
            tree.handle_node(db, node, shapes);
        }
        
        tree.finish();
        tree
    }
    
    fn handle_node(&mut self, db: &dyn Db, node: Node<'_>, shapes: &TagShapes) {
        match node {
            Node::Tag { name, bits, span } => {
                self.handle_tag(db, name, bits, span, shapes);
            }
            Node::Comment { span, .. } => {
                self.blocks.add_leaf(self.active_segment(), "<comment>".into(), span);
            }
            Node::Variable { span, .. } => {
                self.blocks.add_leaf(self.active_segment(), "<var>".into(), span);
            }
            Node::Text { span } => {
                self.blocks.add_leaf(self.active_segment(), "<text>".into(), span);
            }
            Node::Error { full_span, error, .. } => {
                self.blocks.add_leaf(self.active_segment(), error.to_string(), full_span);
            }
        }
    }
    
    fn handle_tag(&mut self, db: &dyn Db, name: TagName, bits: Vec<TagBit>, span: Span, shapes: &TagShapes) {
        let tag_name = name.text(db);
        
        match self.shapes.classify(tag_name) {
            TagClass::Opener { shape } => {
                let parent = self.active_segment();
                let (container, segment) = self.blocks().add_block(parent, tag_name, span);
                
                self.stack.push(TreeFrame {
                    opener_name: tag_name.to_string(),
                    opener_bits: bits,
                    opener_span: span,
                    container_body: container,
                    segment_body: segment,
                    parent_body: parent,
                });
            }
            
            TagClass::Closer { opener_name } => {
                // Find the matching frame
                if let Some(frame_idx) = self.find_frame(&opener_name) {
                    // Pop any unclosed blocks above this one
                    while self.stack.len() > frame_idx + 1 {
                        if let Some(unclosed) = self.stack.pop() {
                            self.blocks().add_error(
                                unclosed.parent_body,
                                format!("Unclosed block '{}'", unclosed.opener_name),
                                unclosed.opener_span,
                            );
                        }
                    }
                    
                    // Now validate and close
                    let frame = self.stack.pop().unwrap();
                    match self.shapes.validate_close(&opener_name, &frame.opener_bits, &bits) {
                        CloseValidation::Valid => {
                            self.blocks().extend(frame.container_body, span);
                        }
                        CloseValidation::ArgumentMismatch { arg, expected, got } => {
                            self.blocks().add_error(
                                frame.segment_body,
                                format!("Argument '{}' mismatch: expected '{}', got '{}'", arg, expected, got),
                                span,
                            );
                            self.stack.push(frame); // Restore frame
                        }
                        // ... handle other validation failures
                    }
                } else {
                    self.blocks().add_error(
                        self.active_segment(),
                        format!("Unexpected closing tag '{}'", tag_name),
                        span,
                    );
                }
            }
            
            TagClass::Intermediate { possible_openers } => {
                self.add_intermediate(tag_name, &possible_openers, span);
            }
            
            TagClass::Unknown => {
                // Treat as leaf
                self.blocks.add_leaf(self.active_segment(), tag_name.to_string(), span);
            }
        }
    }
    
    fn close_block(&mut self, opener_name: &str, closer_bits: &[TagBit], span: Span, shapes: &TagShapes) {
        // Find the matching frame
        if let Some(frame_idx) = self.find_frame(opener_name) {
            // Pop any unclosed blocks above this one
            while self.stack.len() > frame_idx + 1 {
                if let Some(unclosed) = self.stack.pop() {
                    self.blocks.add_error(
                        unclosed.parent_body,
                        format!("Unclosed block '{}'", unclosed.opener_name),
                        unclosed.opener_span,
                    );
                }
            }
            
            // Now validate and close
            let frame = self.stack.pop().unwrap();
            match shapes.validate_close(opener_name, &frame.opener_bits, closer_bits) {
                CloseValidation::Valid => {
                    self.blocks.extend(frame.container_body, span);
                }
                CloseValidation::ArgumentMismatch { arg, expected, got } => {
                    self.blocks.add_error(
                        frame.segment_body,
                        format!("Argument '{}' mismatch: expected '{}', got '{}'", arg, expected, got),
                        span,
                    );
                    self.stack.push(frame); // Restore frame
                }
                CloseValidation::MissingRequiredArg { arg, expected } => {
                    self.blocks.add_error(
                        frame.segment_body,
                        format!("Missing required argument '{}': expected '{}'", arg, expected),
                        span,
                    );
                    self.stack.push(frame);
                }
                CloseValidation::UnexpectedArg { arg, got } => {
                    self.blocks.add_error(
                        frame.segment_body,
                        format!("Unexpected argument '{}' with value '{}'", arg, got),
                        span,
                    );
                    self.stack.push(frame);
                }
                CloseValidation::NotABlock => {
                    // Should not happen as we already classified it
                    self.blocks.add_error(
                        self.active_segment(),
                        format!("Internal error: {} is not a block", opener_name),
                        span,
                    );
                }
            }
        } else {
            self.blocks.add_error(
                self.active_segment(),
                format!("Unexpected closing tag '{}'", opener_name),
                span,
            );
        }
    }
    
    fn add_intermediate(&mut self, tag_name: &str, possible_openers: &[String], span: Span) {
        if let Some(frame) = self.stack.last_mut() {
            if possible_openers.contains(&frame.opener_name) {
                // Add new segment
                frame.segment_body = self.blocks
                    .add_segment(frame.container_body, tag_name.to_string(), span);
            } else {
                self.blocks.add_error(
                    frame.segment_body,
                    format!("'{}' is not valid in '{}'", tag_name, frame.opener_name),
                    span,
                );
            }
        } else {
            self.blocks.add_error(
                self.active_segment(),
                format!("Intermediate tag '{}' outside of block", tag_name),
                span,
            );
        }
    }
    
    fn finish(&mut self) {
        // Close any remaining open blocks
        while let Some(frame) = self.stack.pop() {
            // Check if this block's end tag was optional
            if let Some(shape) = self.shapes.get(&frame.opener_name) {
                if let TagShape::Block { end, .. } = shape {
                    if end.optional {
                        self.blocks.extend(frame.container_body, frame.opener_span);
                    } else {
                        self.blocks.add_error(
                            frame.parent_body,
                            format!("Unclosed block '{}'", frame.opener_name),
                            frame.opener_span,
                        );
                    }
                }
            }
        }
        // Clear the stack as we're done building
        self.stack.clear();
    }
    
    fn find_frame(&self, opener_name: &str) -> Option<usize> {
        self.stack.iter()
            .rposition(|f| f.opener_name == opener_name)
    }
    
    fn active_segment(&self) -> BlockId {
        self.stack
            .last()
            .map(|frame| frame.segment_body)
            .unwrap_or(self.roots[0])
    }
}
```

### Layer 5: Update Builtin Definitions

Tags that need end tag args added:

1. **block** - ✅ Already has `name` arg on endblock
2. **blocktrans/blocktranslate** - Need optional `context` arg on close
3. **cache** - Need optional `cache_key` arg on close
4. **filter** - Need optional filter expression on close

Example for blocktrans:
```rust
"blocktrans" => TagSpec {
    args: &[
        TagArg::String { name: "context", required: false },
        // ... other args
    ],
    end_tag: Some(EndTag {
        name: "endblocktrans",
        args: &[
            TagArg::String { name: "context", required: false },
        ],
    }),
}
```

## Benefits of This Design

1. **Simple and obvious**: End tags have args, if they match we validate them
2. **No magic**: No inference, no special policies, just data
3. **Extensible**: Users can define custom tags with any matching behavior
4. **Fast**: Pre-computed indices mean O(1) lookups everywhere
5. **Type-safe**: Rich enums make invalid states unrepresentable
6. **Single source of truth**: Behavior defined once in the spec
7. **No unnecessary layers**: BlockTree builds itself, no separate builder needed
8. **Clear separation**: TagShapes handle knowledge/validation, BlockTree handles construction

## Migration Path

1. **Phase 1**: Rewrite `shapes.rs` with new structures
   - Delete `EndPolicy` enum and related code
   - Delete `build_end_index` function
   - Implement new `TagShapes` with pre-computed indices
   - Implement `classify()` and `validate_close()` methods
   - Create proper `From<&TagSpecs>` implementation

2. **Phase 2**: Merge builder into `tree.rs`
   - Delete `BlockTreeBuilder` struct entirely
   - Move building logic directly into `BlockTree` impl
   - Add `stack: Vec<TreeFrame>` to BlockTree (cleared after build)
   - Simplify `TreeFrame` to store opener bits for validation
   - Replace complex matching logic with `shapes.validate_close()`
   - Remove `try_close_tag` and `try_handle_intermediate` methods

3. **Phase 3**: Update builtin definitions
   - Add end tag args where needed (blocktrans, cache, filter)
   - Test with real Django templates
   - Verify backwards compatibility with existing templates

## Example: How block/endblock Works

1. User defines in YAML:
   ```yaml
   - name: block
     args:
       - name: block_name
         type: literal
     end_tag:
       name: endblock
       args:
         - name: block_name
           type: literal
           required: false
   ```

2. TagShapes pre-computes:
   - `shapes["block"]` = Block shape with end.match_args containing "block_name"
   - `closer_to_opener["endblock"]` = "block"

3. When parser sees `{% block sidebar %}`:
   - `shapes.classify("block")` → `TagClass::Opener`
   - BlockTree pushes frame with opener_bits = ["sidebar"]

4. When parser sees `{% endblock sidebar %}`:
   - `shapes.classify("endblock")` → `TagClass::Closer { opener_name: "block" }`
   - BlockTree finds "block" frame
   - `shapes.validate_close("block", ["sidebar"], ["sidebar"])` → `Valid`
   - Block closes successfully

5. If parser sees `{% endblock content %}` instead:
   - `shapes.validate_close("block", ["sidebar"], ["content"])` → `ArgumentMismatch`
   - Error: "Argument 'block_name' mismatch: expected 'sidebar', got 'content'"

## Why No Separate Builder?

The original `BlockTreeBuilder` was introduced to manage complexity, but it actually added unnecessary indirection:

1. **No complex configuration needed**: The build process is deterministic - parse nodes, classify tags, manage stack
2. **No reuse**: We never need to build multiple trees with the same builder
3. **Simpler ownership**: BlockTree owns its entire construction lifecycle
4. **Less state passing**: No need to pass tree to builder and back
5. **More cohesive**: All BlockTree logic stays together

The builder pattern makes sense when you have complex configuration or need to reuse builders. Here, it was just an extra layer that made the code harder to follow.

## Conclusion

This redesign eliminates unnecessary complexity by:
1. **Removing intermediate layers**: No BlockTreeBuilder, no EndPolicy enum
2. **Making data drive behavior**: End tag args naturally determine matching requirements
3. **Pre-computing relationships**: TagShapes builds indices once, queries are O(1)
4. **Leveraging Rust's type system**: Rich enums and pattern matching prevent invalid states

The key insight is that Django template tags already have a consistent pattern - end tags can take arguments just like open tags. By embracing this pattern in our data model and eliminating unnecessary abstractions, we get a cleaner, more maintainable, and more powerful system.