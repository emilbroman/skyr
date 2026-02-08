function (doc) {
  emit(doc.name, {
    timestamp: new Date(doc.timestamp).getTime(),
    doc_id: doc._id,
    name: doc.name,
    hash: doc.hash || null,
    symbolic_target: doc.symbolic_target || null,
    action: doc.action,
  });
}
