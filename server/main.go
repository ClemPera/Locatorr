package main

import (
	"context"
	"fmt"
	"net/http"
	"os"

	"github.com/gin-gonic/gin"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/joho/godotenv"
)

func main() {
	//Load env
	if err := godotenv.Load(); err != nil {
		panic(fmt.Sprintf("env file error: %s", err))
	}

	//Setup db
	ctx := context.Background()

	pool, err := pgxpool.New(ctx, os.Getenv("DATABASE_URL"))
	if err != nil {
		panic(fmt.Sprintf("db connection error: %s", err))
	}
	defer pool.Close()

	//Create schema if they don't already exists
	//TODO: make migrations system with goose or maually
	schema, err := os.ReadFile("schema.sql")
	if err != nil {
		panic(fmt.Sprintf("Could not read schema file: %s", err))

	}
	_, err = pool.Exec(ctx, string(schema))
	if err != nil {
		panic(fmt.Sprintf("Could not convert execute schema file: %s", err))

	}

	//Setup routing
	router := gin.Default()
	router.POST("/rendezvous", rendezvous)

	s := &http.Server{
		Addr:    ":9191",
		Handler: router,
	}
	s.ListenAndServe()
}

func rendezvous(c *gin.Context) {
	// TODO: Should only give id and ttl and the client make the rest

}
