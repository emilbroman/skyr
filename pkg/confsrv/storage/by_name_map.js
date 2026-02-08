function (doc) {
  emit(doc.name, {
    timestamp: new Date(doc.timestamp).getTime(),
    doc_id: doc._id,
  });
}
