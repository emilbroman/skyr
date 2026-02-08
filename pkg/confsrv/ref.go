package confsrv

import (
	"time"

	"github.com/go-git/go-git/v5/plumbing"
)

type RefAction string

const (
	RefActionSet   = RefAction("SET")
	RefActionUnset = RefAction("UNSET")
)

type RefDoc struct {
	ID  string `json:"_id,omitempty"`
	Rev string `json:"_rev,omitempty"`

	Action    RefAction `json:"action"`
	Timestamp time.Time `json:"timestamp"`

	Name           plumbing.ReferenceName `json:"name"`
	Hash           string                 `json:"hash,omitempty"`
	SymbolicTarget plumbing.ReferenceName `json:"symbolic_target,omitempty"`
}

func createRefDoc(newRef *plumbing.Reference) (doc *RefDoc) {
	doc = &RefDoc{
		Timestamp: time.Now().In(time.UTC),
		Name:      newRef.Name(),
	}

	if newRef.Type() == plumbing.HashReference {
		doc.Hash = newRef.Hash().String()
	} else {
		doc.SymbolicTarget = newRef.Target()
	}

	return
}

func CreateSetRefDoc(newRef *plumbing.Reference) (doc *RefDoc) {
	doc = createRefDoc(newRef)
	doc.Action = RefActionSet
	return
}

func CreateUnsetRefDoc(newRef *plumbing.Reference) (doc *RefDoc) {
	doc = createRefDoc(newRef)
	doc.Action = RefActionUnset
	return
}

func (r *RefDoc) ToReference() *plumbing.Reference {
	if r.Hash != "" {
		return plumbing.NewHashReference(r.Name, plumbing.NewHash(r.Hash))
	}
	return plumbing.NewSymbolicReference(r.Name, r.SymbolicTarget)
}
