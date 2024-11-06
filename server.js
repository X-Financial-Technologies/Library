const express = require('express');
const fs = require('fs');
const path = require('path');
const app = express();

app.use(express.static('.'));

app.get('/api/files', (req, res) => {
    const directoryPath = './';
    
    fs.readdir(directoryPath, (err, files) => {
        if (err) {
            return res.status(500).send('Error reading directory');
        }
        
        const fileList = files.filter(file => 
            file !== 'server.js' && 
            file !== 'script.js' && 
            file !== 'index.html' &&
            file !== 'node_modules'
        );
        
        res.json(fileList);
    });
});

const PORT = 3000;
app.listen(PORT, () => {
    console.log(`Server running on http://localhost:${PORT}`);
});